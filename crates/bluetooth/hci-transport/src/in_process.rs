//! Bounded in-process Host/Controller endpoints and HCI epoch authority.

use core::{cell::RefCell, convert::Infallible, fmt, future::poll_fn, task::Poll};

use bt_hci::{
    FromHciBytes, FromHciBytesError, PacketKind, ReadHciError,
    cmd::controller_baseband::HostNumberOfCompletedPackets,
    data::{AclPacket, IsoPacket, SyncPacket},
    param::{ConnHandle, ConnHandleCompletedPackets},
    transport::{PacketToController, PacketToHost, Transport},
};

use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    waitqueue::WakerRegistration,
};

use embedded_io::{ErrorKind, ErrorType, ReadExactError, Write};

use oer_bluetooth_hci::{
    ControllerToHostQueueError, HciCommandPacket, HostToControllerFrame,
    validate_controller_to_host_packet,
};

use crate::packet::PacketSlot;

mod codec;
mod queue;
use codec::{
    controller_slot, decode_controller_slot, decode_host_slot, encode_host_packet,
    require_profile_buffer,
};
use queue::{AsyncPacketQueue, PacketQueueEpoch};

/// Opaque identity of one live in-process HCI resource epoch.
///
/// The marker can be copied for affinity checks but cannot be constructed by
/// callers. Its borrow prevents identity from outliving the backing channel.
#[derive(Clone, Copy)]
pub struct HciEpochIdentity<'epoch> {
    marker: &'epoch u8,
    generation: u64,
}

impl HciEpochIdentity<'_> {
    /// Whether two endpoints originate from the same channel and generation.
    pub fn same_epoch(self, other: HciEpochIdentity<'_>) -> bool {
        core::ptr::eq(self.marker, other.marker) && self.generation == other.generation
    }
}

/// An error at the bounded in-process HCI boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HciChannelError {
    /// The supervisor permanently closed this transport direction. Queued
    /// packets remain readable; an empty closed queue cannot receive more.
    Closed,
    /// A non-blocking send found no free packet slot.
    Full,
    /// A non-blocking receive found no published packet.
    Empty,
    /// A packet was offered on the wrong directional half of HCI.
    InvalidDirection,
    /// The packet body exceeds the statically selected storage profile.
    PacketTooLong {
        /// Supplied or declared packet length.
        length: usize,
        /// Maximum packet length retained by this channel.
        capacity: usize,
    },
    /// The caller buffer cannot retain this channel's complete packet profile.
    DestinationTooSmall {
        /// Required caller-buffer length.
        required: usize,
        /// Supplied caller-buffer length.
        available: usize,
    },
    /// A packet header or value is not valid HCI.
    InvalidPacket(FromHciBytesError),
    /// Bytes exist after the payload length declared by the HCI header.
    TrailingBytes,
    /// A `PacketToController` wrote beyond the selected storage profile.
    SerializationOverflow {
        /// Maximum writable packet body length.
        capacity: usize,
    },
    /// A packet's `WriteHci::size` contract disagreed with the bytes it wrote.
    SerializationLengthMismatch {
        /// Length reported before serialization.
        declared: usize,
        /// Bytes actually serialized.
        written: usize,
    },
    /// Safe code observed a retained packet that no longer satisfies admission.
    CorruptRetainedPacket,
}

impl fmt::Display for HciChannelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl core::error::Error for HciChannelError {}

impl From<ReadHciError<Infallible>> for HciChannelError {
    fn from(error: ReadHciError<Infallible>) -> Self {
        match error {
            ReadHciError::BufferTooSmall | ReadHciError::Read(ReadExactError::UnexpectedEof) => {
                Self::InvalidPacket(FromHciBytesError::InvalidSize)
            }
            ReadHciError::InvalidValue => Self::InvalidPacket(FromHciBytesError::InvalidValue),
            ReadHciError::Read(ReadExactError::Other(never)) => match never {},
        }
    }
}

impl embedded_io::Error for HciChannelError {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::Full | Self::Empty => ErrorKind::Other,
            Self::Closed => ErrorKind::BrokenPipe,
            Self::InvalidDirection => ErrorKind::InvalidInput,
            Self::PacketTooLong { .. }
            | Self::DestinationTooSmall { .. }
            | Self::SerializationOverflow { .. } => ErrorKind::OutOfMemory,
            Self::InvalidPacket(_)
            | Self::TrailingBytes
            | Self::SerializationLengthMismatch { .. }
            | Self::CorruptRetainedPacket => ErrorKind::InvalidData,
        }
    }
}

impl From<FromHciBytesError> for HciChannelError {
    fn from(error: FromHciBytesError) -> Self {
        Self::InvalidPacket(error)
    }
}

/// Why a graceful transport retirement cannot complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HciRetirementError {
    /// Host commands, ACL data or credit returns still await consumption.
    HostPacketsPending,
    /// Published Controller packets still await Host consumption.
    ControllerPacketsPending,
    /// Terminal closure already occurred; it cannot become graceful retirement.
    Closed,
}

/// Why a retired transport did not restart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HciRestartError {
    /// The proof or queue generation belongs to another epoch.
    EpochMismatch,
    /// Both original queues must remain empty and closed.
    NotRetired,
    /// Epoch identities cannot wrap and make an old Host handle valid again.
    GenerationExhausted,
}

/// Proof that both queues of one epoch were empty when admission closed.
///
/// It proves nothing about radio or Controller quiescence.
#[must_use = "a retired epoch restarts only with its own proof"]
pub struct HciRetired<'channel> {
    identity: HciEpochIdentity<'channel>,
}

impl HciRetired<'_> {
    /// Whether this proof belongs to the observed epoch.
    pub fn matches_epoch(&self, epoch: HciEpochIdentity<'_>) -> bool {
        self.identity.same_epoch(epoch)
    }
}

/// Two bounded packet queues joining an HCI Host and one raw Controller owner.
///
/// Packet indicators are retained as typed [`PacketKind`] values and are never
/// serialized as UART/H4 bytes. Calling [`Self::split`] requires exclusive
/// access, so safe code cannot manufacture a second endpoint pair while the
/// first pair is alive. `M` selects the synchronization domain; a platform may
/// use a critical-section mutex for IRQ/task handoff without introducing an
/// RTOS.
pub(crate) struct InProcessHciChannel<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    identity: u8,
    host_to_controller: AsyncPacketQueue<M, HOST_TO_CONTROLLER_DEPTH, PACKET_CAPACITY>,
    controller_to_host: AsyncPacketQueue<M, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>,
}

impl<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> InProcessHciChannel<M, HOST_TO_CONTROLLER_DEPTH, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>
where
    M: RawMutex,
{
    /// Construct an empty channel without allocator or runtime registration.
    pub(crate) const fn new() -> Self {
        assert!(
            HOST_TO_CONTROLLER_DEPTH > 0,
            "the Host-to-Controller channel needs a packet slot"
        );
        assert!(
            CONTROLLER_TO_HOST_DEPTH > 0,
            "the Controller-to-Host channel needs a packet slot"
        );
        assert!(
            PACKET_CAPACITY > 0,
            "an HCI channel packet slot must retain at least one byte"
        );
        Self {
            identity: 0,
            host_to_controller: AsyncPacketQueue::new(),
            controller_to_host: AsyncPacketQueue::new(),
        }
    }

    /// Split into the Host transport and raw Controller transport.
    pub(crate) fn split(
        &mut self,
    ) -> (
        InProcessHciHostTransport<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        InProcessHciControllerTransport<
            '_,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
    ) {
        (
            InProcessHciHostTransport {
                host_to_controller: self.host_to_controller.epoch(),
                controller_to_host: self.controller_to_host.epoch(),
            },
            InProcessHciControllerTransport {
                identity: HciEpochIdentity {
                    marker: &self.identity,
                    generation: self.host_to_controller.epoch().generation(),
                },
                host_to_controller: self.host_to_controller.epoch(),
                controller_to_host: self.controller_to_host.epoch(),
            },
        )
    }

    /// Whether the channel remains open and no packet has entered either direction.
    ///
    /// Draining a packet cannot make the channel pristine again. Lifecycle
    /// owners use this monotonic observation before binding the channel to a
    /// powered Controller epoch.
    pub(crate) fn is_pristine(&self) -> bool {
        self.host_to_controller.is_pristine() && self.controller_to_host.is_pristine()
    }
}

impl<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> Default
    for InProcessHciChannel<M, HOST_TO_CONTROLLER_DEPTH, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>
where
    M: RawMutex,
{
    fn default() -> Self {
        Self::new()
    }
}

/// Host-facing packet transport accepted by `bt_hci::ExternalController`.
///
/// Writes await bounded capacity and reads await a Controller publication.
/// Dropping either pending future leaves both queues unchanged. Terminal closure
/// wakes transport waiters, rejects new writes and permits queued reads before
/// returning [`HciChannelError::Closed`]. An external event loop must propagate
/// transport errors to any higher-level command waiters it owns.
pub struct InProcessHciHostTransport<
    'channel,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    host_to_controller: PacketQueueEpoch<'channel, M, HOST_TO_CONTROLLER_DEPTH, PACKET_CAPACITY>,
    controller_to_host: PacketQueueEpoch<'channel, M, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>,
}

/// Write-only Host authority for returning Controller-to-Host ACL credits.
///
/// `Host Number Of Completed Packets` has no success event, so it cannot use
/// the command/response slots in `bt_hci::ExternalController`. This restricted
/// sender retains only the authority needed for that response-less command and
/// shares the same bounded Host-to-Controller queue as the standard facade.
pub struct LeHostAclCreditSender<
    'channel,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    host_to_controller: PacketQueueEpoch<'channel, M, HOST_TO_CONTROLLER_DEPTH, PACKET_CAPACITY>,
}

impl<
    'channel,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    InProcessHciHostTransport<
        'channel,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Derive the restricted response-less ACL credit authority for this epoch.
    pub fn acl_credit_sender(
        &self,
    ) -> LeHostAclCreditSender<'channel, M, HOST_TO_CONTROLLER_DEPTH, PACKET_CAPACITY> {
        LeHostAclCreditSender {
            host_to_controller: self.host_to_controller,
        }
    }
}

impl<M, const HOST_TO_CONTROLLER_DEPTH: usize, const PACKET_CAPACITY: usize>
    LeHostAclCreditSender<'_, M, HOST_TO_CONTROLLER_DEPTH, PACKET_CAPACITY>
where
    M: RawMutex,
{
    /// Return credits for exact connection handles without awaiting an event.
    pub async fn return_completed_packets(
        &self,
        completed: &[ConnHandleCompletedPackets],
    ) -> Result<(), HciChannelError> {
        let command = HostNumberOfCompletedPackets::new(completed);
        let slot = encode_host_packet::<_, PACKET_CAPACITY>(&command)?;
        self.host_to_controller.send(slot).await?;
        Ok(())
    }
}

impl<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> ErrorType
    for InProcessHciHostTransport<
        '_,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    type Error = HciChannelError;
}

impl<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> Transport
    for InProcessHciHostTransport<
        '_,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    async fn read<'buffer, P: PacketToHost<'buffer>>(
        &self,
        buffer: &'buffer mut [u8],
    ) -> Result<P, Self::Error> {
        require_profile_buffer::<PACKET_CAPACITY>(buffer.len())?;
        let slot = self.controller_to_host.receive().await?;
        decode_controller_slot(slot, buffer)
    }

    async fn write<T: PacketToController>(&self, value: &T) -> Result<(), Self::Error> {
        let slot = encode_host_packet::<T, PACKET_CAPACITY>(value)?;
        self.host_to_controller.send(slot).await?;
        Ok(())
    }
}

/// Raw Controller half of the bounded in-process HCI boundary.
///
/// The Controller owner receives Host commands and data here and publishes
/// complete validated events and data. The transport implements no HCI
/// command semantics.
pub struct InProcessHciControllerTransport<
    'channel,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    identity: HciEpochIdentity<'channel>,
    host_to_controller: PacketQueueEpoch<'channel, M, HOST_TO_CONTROLLER_DEPTH, PACKET_CAPACITY>,
    controller_to_host: PacketQueueEpoch<'channel, M, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>,
}

impl<
    'channel,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
>
    InProcessHciControllerTransport<
        'channel,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >
where
    M: RawMutex,
{
    /// Identity shared only by endpoints split from this exact channel epoch.
    pub const fn epoch_identity(&self) -> HciEpochIdentity<'channel> {
        self.identity
    }

    /// Permanently close both packet directions without discarding their FIFOs.
    pub fn close(&self) {
        self.host_to_controller.close();
        self.controller_to_host.close();
    }

    /// Wait until both FIFOs are empty, or report terminal closure.
    ///
    /// The observation reserves nothing; a later packet can invalidate it.
    pub fn wait_retirement_ready(
        &self,
    ) -> impl core::future::Future<Output = Result<(), HciRetirementError>>
    + use<'channel, M, HOST_TO_CONTROLLER_DEPTH, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY> {
        self.host_to_controller
            .wait_drained_with(self.controller_to_host)
    }

    /// Close empty FIFOs atomically and return the retirement proof.
    pub fn try_retire(&self) -> Result<HciRetired<'channel>, HciRetirementError> {
        self.host_to_controller
            .try_retire_with(self.controller_to_host)?;
        Ok(HciRetired {
            identity: self.identity,
        })
    }

    /// Start a fresh epoch on the same storage from its retirement proof.
    ///
    /// Existing Host handles and pending futures stay closed forever; only
    /// the returned Host uses the new generation.
    #[allow(
        clippy::type_complexity,
        clippy::result_large_err,
        reason = "rejection returns the unconsumed proof"
    )]
    pub fn restart(
        &mut self,
        retired: HciRetired<'channel>,
    ) -> Result<
        InProcessHciHostTransport<
            'channel,
            M,
            HOST_TO_CONTROLLER_DEPTH,
            CONTROLLER_TO_HOST_DEPTH,
            PACKET_CAPACITY,
        >,
        (HciRestartError, HciRetired<'channel>),
    > {
        if !retired.matches_epoch(self.identity) {
            return Err((HciRestartError::EpochMismatch, retired));
        }
        let (incoming, outgoing) = match self
            .host_to_controller
            .restart_with(self.controller_to_host)
        {
            Ok(queues) => queues,
            Err(error) => return Err((error, retired)),
        };
        self.host_to_controller = incoming;
        self.controller_to_host = outgoing;
        self.identity.generation = incoming.generation();
        Ok(InProcessHciHostTransport {
            host_to_controller: incoming,
            controller_to_host: outgoing,
        })
    }

    /// Await and consume the oldest complete Host packet.
    pub async fn receive<'buffer>(
        &self,
        buffer: &'buffer mut [u8],
    ) -> Result<HostToControllerFrame<'buffer>, HciChannelError> {
        require_profile_buffer::<PACKET_CAPACITY>(buffer.len())?;
        let slot = self.host_to_controller.receive().await?;
        decode_host_slot(slot, buffer)
    }

    /// Consume a Host packet immediately or return [`HciChannelError::Empty`].
    pub fn try_receive<'buffer>(
        &self,
        buffer: &'buffer mut [u8],
    ) -> Result<HostToControllerFrame<'buffer>, HciChannelError> {
        require_profile_buffer::<PACKET_CAPACITY>(buffer.len())?;
        let slot = self.host_to_controller.try_receive()?;
        decode_host_slot(slot, buffer)
    }

    /// Consume the oldest admitted Host packet: a command, or ACL data whose
    /// connection handle `admit_acl` admits. Commands and every handle's
    /// data each keep their FIFO order; data of a refused handle waits
    /// without holding back the others.
    pub fn try_receive_admitted<'buffer>(
        &self,
        buffer: &'buffer mut [u8],
        admit_acl: impl Fn(ConnHandle) -> bool,
    ) -> Result<HostToControllerFrame<'buffer>, HciChannelError> {
        require_profile_buffer::<PACKET_CAPACITY>(buffer.len())?;
        let slot = self.host_to_controller.try_receive_admitted(admit_acl)?;
        decode_host_slot(slot, buffer)
    }

    /// Wait until a packet is available, or the queue is terminally closed.
    ///
    /// This reserves and consumes nothing; finish with [`Self::try_receive`].
    pub async fn wait_receive_ready(&self) {
        self.host_to_controller.wait_receive_ready().await;
    }

    /// Wait until an admitted packet is available under `admit_acl`.
    pub async fn wait_receive_admitted(&self, admit_acl: impl Fn(ConnHandle) -> bool) {
        self.host_to_controller
            .wait_receive_admitted(admit_acl)
            .await;
    }

    /// Validate and asynchronously publish one Controller packet.
    pub async fn publish(&self, kind: PacketKind, bytes: &[u8]) -> Result<(), HciChannelError> {
        let slot = controller_slot::<PACKET_CAPACITY>(kind, bytes)?;
        self.controller_to_host.send(slot).await?;
        Ok(())
    }

    /// Wait until Controller-to-Host storage has capacity or is terminally closed.
    ///
    /// This reserves no slot; finish with [`Self::try_publish`].
    pub async fn wait_publish_ready(&self) {
        self.controller_to_host.wait_send_ready().await;
    }

    /// Publish immediately or return [`HciChannelError::Full`] without overwrite.
    pub fn try_publish(&self, kind: PacketKind, bytes: &[u8]) -> Result<(), HciChannelError> {
        let slot = controller_slot::<PACKET_CAPACITY>(kind, bytes)?;
        self.controller_to_host.try_send(slot)
    }
}

#[cfg(test)]
mod tests;
