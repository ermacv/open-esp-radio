//! Owned packet boundary used by the Xarxa/Embassy cutover.
//!
//! This module contains no Wi-Fi scheduler. It moves complete [`PacketBuf`]
//! owners between the network and radio execution domains. RX goes through a
//! bounded queue; TX goes from the stack's sending path straight to the
//! device's transmit half, which holds the only queue on its way.

use core::task::{Context, Poll, Waker};
use core::{
    marker::PhantomData,
    sync::atomic::{AtomicU32, Ordering},
};

use oer_portable_memory::{
    Entered, ExecutionContext, WakingSpscQueue, WakingSpscReceiver, WakingSpscSender,
};
use oer_portable_sync::PriorityWaker;
use xarxa_driver::{
    Capabilities, ChecksumCapabilities, Driver, HardwareAddress, LinkState as DriverLinkState,
    NotSupported, PacketBuf, PacketBufAllocator, PacketPoolWaiter,
};

use crate::{FrameLengthError, NetTransmit, NetworkInterfaceId, RxEnqueueError, TransmitError};
use oer_protocols_network_wire::ethernet::{
    EtherType, EthernetHeader, HEADER_LEN as ETHERNET_HEADER_LEN,
};

#[derive(Clone, Copy)]
struct LinkSnapshot {
    epoch: u32,
    up: bool,
}

/// Link lifetime shared by the network producer/consumer pair.
///
/// Bit zero is the level state. The remaining bits form a generation which is
/// advanced on every Down -> Up transition. Queue entries carry that
/// generation, so an owner published concurrently with teardown can never be
/// retargeted to the next association lifetime.
struct OwnedLinkState {
    state: AtomicU32,
    network_waker: PriorityWaker,
    radio_waker: PriorityWaker,
}

impl OwnedLinkState {
    const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
            network_waker: PriorityWaker::new(),
            radio_waker: PriorityWaker::new(),
        }
    }

    fn snapshot(&self) -> LinkSnapshot {
        let state = self.state.load(Ordering::Acquire);
        LinkSnapshot {
            epoch: state >> 1,
            up: state & 1 != 0,
        }
    }

    fn set(&self, up: bool) {
        let mut current = self.state.load(Ordering::Acquire);
        loop {
            if (current & 1 != 0) == up {
                return;
            }
            let next = if up {
                current.wrapping_add(2) | 1
            } else {
                current & !1
            };
            match self.state.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.network_waker.wake();
                    self.radio_waker.wake();
                    return;
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn register_network_waker(&self, waker: &Waker) {
        self.network_waker.register(waker);
    }

    fn register_radio_waker(&self, waker: &Waker) {
        self.radio_waker.register(waker);
    }

    fn wake_network(&self) {
        self.network_waker.wake();
    }
}

struct QueuedPacket {
    epoch: u32,
    packet: PacketBuf,
}

/// Refusal to split one endpoint into its network and radio owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointSplitError {
    /// These resources already lent their unique pair of owners.
    AlreadySplit,
    /// The RX pool's asynchronous radio waiter is already claimed.
    RxWaiterUnavailable,
}

/// Static bounded RX queue for one permanent logical network endpoint.
///
/// Packet bytes do not live in this value. RX owners come from the allocator
/// supplied to [`split`](Self::split).
pub struct OwnedEndpointResources<X: ExecutionContext, const RX_QUEUE_DEPTH: usize> {
    rx: WakingSpscQueue<QueuedPacket, RX_QUEUE_DEPTH>,
    rx_waiter: Option<PacketPoolWaiter>,
    link: OwnedLinkState,
    context: PhantomData<fn() -> X>,
}

impl<X: ExecutionContext, const RX_QUEUE_DEPTH: usize> OwnedEndpointResources<X, RX_QUEUE_DEPTH> {
    /// Create an inactive, empty endpoint with a nonzero queue depth.
    ///
    /// ```compile_fail,E0080
    /// use oer_portable_embassy_net::OwnedEndpointResources;
    /// let _ = OwnedEndpointResources::<oer_portable_memory::Never, 0>::new();
    /// ```
    pub const fn new() -> Self {
        const {
            if RX_QUEUE_DEPTH == 0 {
                panic!("owned endpoint queues must not be empty");
            }
        }
        Self {
            rx: WakingSpscQueue::new(),
            rx_waiter: None,
            link: OwnedLinkState::new(),
            context: PhantomData,
        }
    }

    /// Split one endpoint into its unique network device, which sends
    /// through `transmit`, and radio owner.
    ///
    /// `rx_allocator` should normally point at an internal-SRAM pool reserved
    /// for frames retained by Xarxa. Its slots return to that pool on the final
    /// [`PacketBuf`] drop, independently of which core performs it.
    pub fn split<T: NetTransmit<Frame = OwnedNetworkTxFrame>>(
        &mut self,
        interface: NetworkInterfaceId,
        hardware_address: [u8; 6],
        rx_allocator: PacketBufAllocator,
        transmit: T,
    ) -> Result<
        (
            OwnedNetworkDevice<'_, X, RX_QUEUE_DEPTH, T>,
            OwnedNetworkRunner<'_, X, RX_QUEUE_DEPTH>,
        ),
        EndpointSplitError,
    > {
        if self.rx_waiter.is_some() {
            return Err(EndpointSplitError::AlreadySplit);
        }
        let waiter = rx_allocator
            .try_claim_waiter()
            .ok_or(EndpointSplitError::RxWaiterUnavailable)?;
        // The exclusive resource borrow initializes storage and lends its waiter
        // together with the disjoint queues; no second lookup or latch is needed.
        let rx_waiter = self.rx_waiter.insert(waiter);
        let (sender, receiver) = self.rx.split();
        Ok((
            OwnedNetworkDevice {
                hardware_address,
                rx: receiver,
                tx: transmit,
                waker: None,
                unflushed: false,
                link: &self.link,
                checksum: ChecksumCapabilities::default(),
                local: PhantomData,
            },
            OwnedNetworkRunner {
                interface,
                rx: OwnedRxPublisher {
                    rx: sender,
                    link: &self.link,
                    rx_allocator,
                    rx_waiter,
                    local: PhantomData,
                },
                control: OwnedLinkController {
                    interface,
                    link: &self.link,
                    local: PhantomData,
                },
                local: PhantomData,
            },
        ))
    }
}

impl<X: ExecutionContext, const RX_QUEUE_DEPTH: usize> Default
    for OwnedEndpointResources<X, RX_QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Xarxa/Embassy side of one owned packet endpoint.
///
/// The device stays on the constructing thread even for a shared
/// metadata context; it cannot be sent to another task thread.
/// ```compile_fail,E0277
/// use oer_portable_embassy_net::OwnedNetworkDevice;
/// fn require_send<T: Send>() {}
/// require_send::<OwnedNetworkDevice<'static, oer_portable_memory::Never, 1, ()>>();
/// ```
pub struct OwnedNetworkDevice<'resources, X: ExecutionContext, const RX_QUEUE_DEPTH: usize, T> {
    hardware_address: [u8; 6],
    rx: WakingSpscReceiver<'resources, QueuedPacket, RX_QUEUE_DEPTH>,
    tx: T,
    /// The stack's poll task: a full device wakes it once it has room.
    waker: Option<Waker>,
    /// A burst went to the device and the stack's next poll ends it.
    unflushed: bool,
    link: &'resources OwnedLinkState,
    checksum: ChecksumCapabilities,
    local: PhantomData<Entered<X>>,
}

impl<X: ExecutionContext, const RX_QUEUE_DEPTH: usize, T: NetTransmit<Frame = OwnedNetworkTxFrame>>
    OwnedNetworkDevice<'_, X, RX_QUEUE_DEPTH, T>
{
    /// Ethernet address reported to the network stack.
    pub const fn hardware_address(&self) -> [u8; 6] {
        self.hardware_address
    }

    /// Register before each stack poll for link, RX and TX-capacity changes;
    /// memory the device returned comes back here too, sending or not.
    pub fn register_waker(&mut self, waker: &Waker) {
        if core::mem::take(&mut self.unflushed) {
            self.tx.flush();
        }
        self.link.register_network_waker(waker);
        let _ = self.tx.poll_room(&mut Context::from_waker(waker));
        match &mut self.waker {
            Some(kept) if kept.will_wake(waker) => {}
            kept => *kept = Some(waker.clone()),
        }
    }

    /// Whether this interface currently belongs to an active role lifetime.
    pub fn link_is_up(&self) -> bool {
        self.link.snapshot().up
    }

    /// Override checksum work advertised to Xarxa before stack construction.
    pub fn with_checksum_capabilities(mut self, checksum: ChecksumCapabilities) -> Self {
        self.checksum = checksum;
        self
    }

    /// Claim the next received packet from the current role lifetime.
    pub fn receive(&mut self) -> Option<PacketBuf> {
        loop {
            let queued = self.rx.try_receive()?;
            let current = self.link.snapshot();
            if current.up && current.epoch == queued.epoch {
                return Some(queued.packet);
            }
            // A stale owner returns to its originating RX pool here.
        }
    }

    /// Whether the device takes one frame now. A full device wakes the
    /// stack's poll task once it has room.
    pub fn can_transmit(&mut self) -> bool {
        if !X::running() || !self.link.snapshot().up {
            return false;
        }
        if self.tx.has_room() {
            return true;
        }
        // What the device returned may be the room.
        self.tx.reclaim();
        if self.tx.has_room() {
            return true;
        }
        let Some(waker) = &self.waker else {
            return false;
        };
        self.tx
            .poll_room(&mut Context::from_waker(waker))
            .is_ready()
    }

    /// A frame went to the device: the stack's next poll, on this executor
    /// once the sender yields, ends the burst.
    fn burst(&mut self) {
        if self.unflushed {
            return;
        }
        match &self.waker {
            Some(waker) => {
                self.unflushed = true;
                waker.wake_by_ref();
            }
            None => self.tx.flush(),
        }
    }

    /// Hand one complete Ethernet packet to the device.
    ///
    /// Only a full device returns the unchanged owner: with the unique
    /// mutable device reference, a `true` [`can_transmit`](Self::can_transmit)
    /// result followed immediately by this call does not find one. A
    /// concurrent link-down does not revoke that admission: the packet ends
    /// here, as one lost on air, instead of violating the driver contract;
    /// so does a frame the device refuses for its length.
    pub fn transmit(&mut self, packet: PacketBuf) -> Result<(), PacketBuf> {
        match self.tx.try_transmit(OwnedNetworkTxFrame { packet }) {
            Ok(()) => {
                self.burst();
                Ok(())
            }
            Err(refused) => match refused.reason {
                TransmitError::QueueFull => Err(refused.frame.into_packet()),
                TransmitError::LinkDown | TransmitError::InvalidLength(_) => Ok(()),
            },
        }
    }
}

impl<X: ExecutionContext, const RX_QUEUE_DEPTH: usize, T: NetTransmit<Frame = OwnedNetworkTxFrame>>
    Driver for OwnedNetworkDevice<'_, X, RX_QUEUE_DEPTH, T>
{
    fn capabilities(&self) -> Capabilities {
        let mut capabilities = Capabilities::default();
        capabilities.checksum = self.checksum;
        capabilities
    }

    fn hardware_address(&self) -> HardwareAddress {
        HardwareAddress::Ethernet(self.hardware_address)
    }

    fn link_state(&mut self) -> DriverLinkState {
        if self.link_is_up() {
            DriverLinkState::Up
        } else {
            DriverLinkState::Down
        }
    }

    fn register_waker(&mut self, waker: &Waker) -> Result<(), NotSupported> {
        OwnedNetworkDevice::register_waker(self, waker);
        Ok(())
    }

    fn receive(&mut self) -> Option<PacketBuf> {
        OwnedNetworkDevice::receive(self)
    }

    fn can_transmit(&mut self) -> bool {
        OwnedNetworkDevice::can_transmit(self)
    }

    fn transmit(&mut self, packet: PacketBuf) -> Result<(), PacketBuf> {
        OwnedNetworkDevice::transmit(self, packet)
    }
}

/// One complete Ethernet packet on its way to the device, in the storage the
/// stack wrote it.
pub struct OwnedNetworkTxFrame {
    packet: PacketBuf,
}

/// Affine RX-only capability for a physical datapath service.
///
/// ```compile_fail,E0277
/// use oer_portable_embassy_net::OwnedRxPublisher;
/// fn require_clone<T: Clone>() {}
/// require_clone::<OwnedRxPublisher<'static, 1>>();
/// ```
/// ```compile_fail,E0277
/// use oer_portable_embassy_net::OwnedRxPublisher;
/// fn require_send<T: Send>() {}
/// require_send::<OwnedRxPublisher<'static, 1>>();
/// ```
///
/// The capability can allocate only from this endpoint's dedicated RX pool;
/// it cannot observe or claim network-originated TX owners.
pub struct OwnedRxPublisher<'resources, const RX_QUEUE_DEPTH: usize> {
    rx: WakingSpscSender<'resources, QueuedPacket, RX_QUEUE_DEPTH>,
    link: &'resources OwnedLinkState,
    rx_allocator: PacketBufAllocator,
    rx_waiter: &'resources PacketPoolWaiter,
    local: PhantomData<*mut ()>,
}

impl<const RX_QUEUE_DEPTH: usize> OwnedRxPublisher<'_, RX_QUEUE_DEPTH> {
    /// Number of complete RX owners waiting for Xarxa.
    pub fn queue_len(&self) -> usize {
        RX_QUEUE_DEPTH.saturating_sub(self.rx.free_capacity())
    }

    /// Ready while one more packet fits the stack's RX queue; otherwise the
    /// stack's next take wakes `context`. A lent frame needs no more: its
    /// packet is its own buffer.
    pub fn poll_queue_room(&mut self, context: &mut Context<'_>) -> Poll<()> {
        // Register only on a full queue: a take wakes nobody otherwise.
        if self.rx.free_capacity() != 0 {
            return Poll::Ready(());
        }
        core::pin::pin!(self.rx.ready()).poll(context).map(drop)
    }

    /// Poll until one bounded RX queue entry can be published.
    pub fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<()> {
        self.link.register_radio_waker(context.waker());
        if !self.link.snapshot().up {
            return Poll::Pending;
        }
        {
            let mut ready = core::pin::pin!(self.rx.ready());
            if ready.as_mut().poll(context).is_pending() {
                return Poll::Pending;
            }
        }
        if self.rx_allocator.has_available() {
            return Poll::Ready(());
        }
        self.rx_waiter.register(context.waker());
        if self.rx_allocator.has_available() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    /// Copy one received Ethernet frame into its final Xarxa owner.
    pub fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        if frame.len() < ETHERNET_HEADER_LEN {
            return Err(RxEnqueueError::InvalidLength(FrameLengthError::TooShort));
        }
        let mut packet = self
            .rx_allocator
            .try_alloc()
            .ok_or(RxEnqueueError::PoolExhausted)?;
        if frame.len() > packet.capacity() {
            return Err(RxEnqueueError::InvalidLength(FrameLengthError::TooLong));
        }
        packet.set_len(frame.len());
        for (byte, source) in packet.iter_mut().zip(frame) {
            *byte = *source;
        }
        self.try_publish(packet)
    }

    /// Build Ethernet-II header and payload directly in the final RX owner.
    pub fn try_send_parts(
        &mut self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: &[u8],
    ) -> Result<(), RxEnqueueError> {
        let frame_len = ETHERNET_HEADER_LEN
            .checked_add(payload.len())
            .ok_or(RxEnqueueError::InvalidLength(FrameLengthError::TooLong))?;
        let mut packet = self
            .rx_allocator
            .try_alloc()
            .ok_or(RxEnqueueError::PoolExhausted)?;
        if frame_len > packet.capacity() {
            return Err(RxEnqueueError::InvalidLength(FrameLengthError::TooLong));
        }
        packet.set_len(frame_len);
        let header = EthernetHeader {
            destination,
            source,
            ether_type: EtherType::new(ether_type),
        }
        .to_bytes();
        // Two plain element loops, which the compiler lowers to block
        // copies: the header, then the payload after it.
        let (head, body) = packet
            .split_at_mut_checked(ETHERNET_HEADER_LEN)
            .ok_or(RxEnqueueError::InvalidLength(FrameLengthError::TooShort))?;
        for (byte, value) in head.iter_mut().zip(header) {
            *byte = value;
        }
        for (byte, value) in body.iter_mut().zip(payload) {
            *byte = *value;
        }
        self.try_publish(packet)
    }

    /// Publish an already-owned RX packet without copying it again.
    pub fn try_publish(&mut self, packet: PacketBuf) -> Result<(), RxEnqueueError> {
        if packet.len() < ETHERNET_HEADER_LEN {
            return Err(RxEnqueueError::InvalidLength(FrameLengthError::TooShort));
        }
        let snapshot = self.link.snapshot();
        if !snapshot.up {
            return Err(RxEnqueueError::LinkDown);
        }
        match self.rx.try_send(QueuedPacket {
            epoch: snapshot.epoch,
            packet,
        }) {
            Ok(()) => {
                self.link.wake_network();
                Ok(())
            }
            Err(_) => Err(RxEnqueueError::QueueFull),
        }
    }
}

impl OwnedNetworkTxFrame {
    /// Complete Ethernet-II bytes.
    pub fn ethernet(&self) -> &[u8] {
        &self.packet
    }

    pub(crate) fn packet(&self) -> &PacketBuf {
        &self.packet
    }

    /// The packet.
    pub(crate) fn into_packet(self) -> PacketBuf {
        self.packet
    }
}

/// Affine link-state capability lent exclusively by the radio-side owner.
/// ```compile_fail,E0277
/// use oer_portable_embassy_net::OwnedLinkController;
/// fn require_copy<T: Copy>() {}
/// require_copy::<OwnedLinkController<'static>>();
/// ```
pub struct OwnedLinkController<'resources> {
    interface: NetworkInterfaceId,
    link: &'resources OwnedLinkState,
    local: PhantomData<*mut ()>,
}

impl OwnedLinkController<'_> {
    /// Permanent logical interface controlled by this capability.
    pub const fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    /// Publish the role's link level. A Down -> Up edge creates a new epoch.
    pub fn set_link_up(&mut self, up: bool) {
        self.link.set(up);
    }
}

/// Sole radio-side owner of one network endpoint.
///
/// An RX loan excludes another RX loan, link changes and moving the runner.
/// ```compile_fail,E0499
/// use oer_portable_embassy_net::OwnedNetworkRunner;
/// use oer_portable_memory::ExecutionContext;
/// fn two<X: ExecutionContext>(runner: &mut OwnedNetworkRunner<'_, X, 1>) {
///     let first = runner.rx_publisher();
///     let second = runner.rx_publisher();
///     drop((first, second));
/// }
/// ```
pub struct OwnedNetworkRunner<'resources, X: ExecutionContext, const RX_QUEUE_DEPTH: usize> {
    interface: NetworkInterfaceId,
    rx: OwnedRxPublisher<'resources, RX_QUEUE_DEPTH>,
    control: OwnedLinkController<'resources>,
    local: PhantomData<Entered<X>>,
}

impl<'resources, X: ExecutionContext, const RX_QUEUE_DEPTH: usize>
    OwnedNetworkRunner<'resources, X, RX_QUEUE_DEPTH>
{
    /// Exclusively lend the endpoint's link controller.
    pub fn link_controller(&mut self) -> &mut OwnedLinkController<'resources> {
        &mut self.control
    }

    /// Permanent logical interface represented by this endpoint.
    pub const fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    /// Exclusively lend the sole RX publisher for this endpoint.
    pub fn rx_publisher(&mut self) -> &mut OwnedRxPublisher<'resources, RX_QUEUE_DEPTH> {
        &mut self.rx
    }

    /// Copy one received Ethernet frame directly into its final Xarxa owner.
    pub fn try_send_rx(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.rx_publisher().try_send(frame)
    }

    /// Publish an already-owned RX packet without copying it again.
    pub fn try_publish_rx(&mut self, packet: PacketBuf) -> Result<(), RxEnqueueError> {
        self.rx_publisher().try_publish(packet)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros, clippy::disallowed_methods)]
mod tests;
