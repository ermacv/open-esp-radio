//! Owned packet boundary used by the Xarxa/Embassy cutover.
//!
//! This module contains no Wi-Fi scheduler. It moves complete [`PacketBuf`]
//! owners between the network and radio execution domains through bounded
//! queues. TX is indexed by Ethernet destination before publication. Radio
//! policy selects a destination before claiming owners and validates peer/TID
//! eligibility before SRAM promotion.

use core::task::{Context, Poll, Waker};
use core::{
    future::{Future, poll_fn},
    marker::PhantomData,
    sync::atomic::{AtomicU32, Ordering},
};

use oer_memory::{
    Entered, ExecutionContext, WakingSpscQueue, WakingSpscReceiver, WakingSpscSender,
};
use oer_sync::PriorityWaker;
use xarxa_driver::{
    Capabilities, ChecksumCapabilities, Driver, HardwareAddress, LinkState as DriverLinkState,
    NotSupported, PacketBuf, PacketBufAllocator, PacketPoolWaiter,
};

use crate::datapath::{DestinationTxHead, DestinationTxQueues};

mod tx_budget;
mod tx_queue;
use tx_budget::TxCredit;
use tx_queue::TxQueue;

use crate::{ETHERNET_HEADER_LEN, FrameLengthError, NetworkInterfaceId, RxEnqueueError};

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

/// Access to endpoint metadata was refused: wrong execution context or nesting.
/// No queued owner is consumed by a refused access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessRefused;

/// Refusal to split one endpoint into its network and radio owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointSplitError {
    /// These resources already lent their unique pair of owners.
    AlreadySplit,
    /// The RX pool's asynchronous radio waiter is already claimed.
    RxWaiterUnavailable,
}

/// Wake a displaced registration only after shared state has been restored.
// The stack analyzer proves Waker calls from their DWARF method scopes.
// Separate fixed-slot wake/drop calls keep scopes that merged calls can lose.
fn wake_replaced(previous: Option<Waker>, current: &Waker) {
    if let Some(previous) = previous {
        if !previous.will_wake(current) {
            previous.wake_by_ref();
            drop(previous);
        } else {
            drop(previous);
        }
    }
}

/// Static bounded queues for one permanent logical network endpoint.
///
/// Packet bytes do not live in this value. RX owners come from the allocator
/// supplied to [`split`](Self::split); TX owners retain the general Xarxa pool
/// selected by the application. `TX_QUEUE_DEPTH` bounds all admitted software
/// owners, including frames retained by the radio after dequeue. Physical DMA
/// storage has its own lifetime and budget.
pub struct OwnedEndpointResources<
    X: ExecutionContext,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    rx: WakingSpscQueue<QueuedPacket, RX_QUEUE_DEPTH>,
    tx: TxQueue<X, TX_QUEUE_DEPTH>,
    rx_waiter: Option<PacketPoolWaiter>,
    link: OwnedLinkState,
}

impl<X: ExecutionContext, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize>
    OwnedEndpointResources<X, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    /// Create an inactive, empty endpoint with nonzero queue depths.
    ///
    /// ```compile_fail,E0080
    /// use oer_embassy_net::OwnedEndpointResources;
    /// let _ = OwnedEndpointResources::<oer_memory::Never, 0, 1>::new();
    /// ```
    /// ```compile_fail,E0080
    /// use oer_embassy_net::OwnedEndpointResources;
    /// let _ = OwnedEndpointResources::<oer_memory::Never, 1, 0>::new();
    /// ```
    pub const fn new() -> Self {
        const {
            if RX_QUEUE_DEPTH == 0 || TX_QUEUE_DEPTH == 0 {
                panic!("owned endpoint queues must not be empty");
            }
        }
        Self {
            rx: WakingSpscQueue::new(),
            tx: TxQueue::new(),
            rx_waiter: None,
            link: OwnedLinkState::new(),
        }
    }

    /// Split one endpoint into its unique network device and radio owner.
    ///
    /// `rx_allocator` should normally point at an internal-SRAM pool reserved
    /// for frames retained by Xarxa. Its slots return to that pool on the final
    /// [`PacketBuf`] drop, independently of which core performs it.
    pub fn split(
        &mut self,
        interface: NetworkInterfaceId,
        hardware_address: [u8; 6],
        rx_allocator: PacketBufAllocator,
    ) -> Result<
        (
            OwnedNetworkDevice<'_, X, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>,
            OwnedNetworkRunner<'_, X, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>,
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
                tx: &self.tx,
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
                tx: &self.tx,
                link: &self.link,
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

impl<X: ExecutionContext, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize> Default
    for OwnedEndpointResources<X, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
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
/// use oer_embassy_net::OwnedNetworkDevice;
/// fn require_send<T: Send>() {}
/// require_send::<OwnedNetworkDevice<'static, oer_memory::Never, 1, 1>>();
/// ```
pub struct OwnedNetworkDevice<
    'resources,
    X: ExecutionContext,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    hardware_address: [u8; 6],
    rx: WakingSpscReceiver<'resources, QueuedPacket, RX_QUEUE_DEPTH>,
    tx: &'resources TxQueue<X, TX_QUEUE_DEPTH>,
    link: &'resources OwnedLinkState,
    checksum: ChecksumCapabilities,
    local: PhantomData<Entered<X>>,
}

impl<X: ExecutionContext, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize>
    OwnedNetworkDevice<'_, X, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    /// Ethernet address reported to the network stack.
    pub const fn hardware_address(&self) -> [u8; 6] {
        self.hardware_address
    }

    /// Register before each stack poll for link, RX and TX-capacity changes.
    pub fn register_waker(&mut self, waker: &Waker) {
        self.link.register_network_waker(waker);
        self.tx.register_sender(waker);
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

    /// Whether the shared software TX budget can admit one packet now.
    pub fn can_transmit(&self) -> bool {
        X::running() && self.link.snapshot().up && !self.tx.is_full()
    }

    /// Transfer one complete Ethernet packet into the radio-owned queue.
    ///
    /// Failure returns the unchanged owner. With the unique mutable device
    /// reference, a `true` [`can_transmit`](Self::can_transmit) result followed
    /// immediately by this call cannot fail because of another producer. A
    /// concurrent link-down does not revoke that admission: the packet is
    /// accepted with the observed epoch and is terminally dropped by the radio
    /// consumer instead of violating the driver contract.
    pub fn transmit(&mut self, packet: PacketBuf) -> Result<(), PacketBuf> {
        let snapshot = self.link.snapshot();
        match self.tx.push(QueuedPacket {
            epoch: snapshot.epoch,
            packet,
        }) {
            Ok(()) => Ok(()),
            Err(queued) => Err(queued.packet),
        }
    }
}

impl<X: ExecutionContext, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize> Driver
    for OwnedNetworkDevice<'_, X, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
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

/// One driver-owned TX packet claimed by the physical radio.
///
/// Its admission credit remains occupied through radio retention and rollback.
/// Dropping the frame returns the packet first, then wakes a blocked producer.
/// Borrowing the endpoint prevents either resource from outliving its storage.
pub struct OwnedNetworkTxFrame<'resources> {
    interface: NetworkInterfaceId,
    packet: PacketBuf,
    // Field order is intentional: return the packet to its pool before waking
    // a producer which can immediately spend this software admission credit.
    _credit: TxCredit<'resources>,
}

/// Affine RX-only capability for a physical datapath service.
///
/// ```compile_fail,E0277
/// use oer_embassy_net::OwnedRxPublisher;
/// fn require_clone<T: Clone>() {}
/// require_clone::<OwnedRxPublisher<'static, 1>>();
/// ```
/// ```compile_fail,E0277
/// use oer_embassy_net::OwnedRxPublisher;
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
        let bytes = destination
            .into_iter()
            .chain(source)
            .chain(ether_type.to_be_bytes())
            .chain(payload.iter().copied());
        for (byte, source) in packet.iter_mut().zip(bytes) {
            *byte = source;
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

impl OwnedNetworkTxFrame<'_> {
    /// Logical VIF which accepted this owner.
    pub const fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    pub const fn tag(&self) -> &NetworkInterfaceId {
        &self.interface
    }

    /// Complete Ethernet-II bytes.
    pub fn ethernet(&self) -> &[u8] {
        &self.packet
    }

    /// Complete Ethernet-II bytes before physical SRAM admission.
    pub fn as_slice(&self) -> &[u8] {
        self.ethernet()
    }
}

/// Affine link-state capability lent exclusively by the radio-side owner.
/// ```compile_fail,E0277
/// use oer_embassy_net::OwnedLinkController;
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
/// use oer_embassy_net::OwnedNetworkRunner;
/// use oer_memory::ExecutionContext;
/// fn two<X: ExecutionContext>(runner: &mut OwnedNetworkRunner<'_, X, 1, 1>) {
///     let first = runner.rx_publisher();
///     let second = runner.rx_publisher();
///     drop((first, second));
/// }
/// ```
/// Publication waits also retain the unique runner borrow.
/// ```compile_fail,E0499
/// use oer_embassy_net::OwnedNetworkRunner;
/// use oer_memory::ExecutionContext;
/// fn two<X: ExecutionContext>(runner: &mut OwnedNetworkRunner<'_, X, 1, 1>) {
///     let first = runner.wait_tx_publication();
///     let second = runner.wait_tx_publication();
///     drop((first, second));
/// }
/// ```
pub struct OwnedNetworkRunner<
    'resources,
    X: ExecutionContext,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    interface: NetworkInterfaceId,
    rx: OwnedRxPublisher<'resources, RX_QUEUE_DEPTH>,
    tx: &'resources TxQueue<X, TX_QUEUE_DEPTH>,
    link: &'resources OwnedLinkState,
    control: OwnedLinkController<'resources>,
    local: PhantomData<Entered<X>>,
}

/// Object-safe radio-side view of one bounded owned TX frontier.
///
/// The trait deliberately exposes no physical memory or scheduler policy. It
/// lets a radio adapter compose the software owner queue with its private SRAM
/// allocator without depending on the queue's compile-time depth.
pub trait OwnedTxFrameSource<'resources>:
    DestinationTxQueues<Frame = OwnedNetworkTxFrame<'resources>, Error = AccessRefused>
{
    fn interface(&self) -> NetworkInterfaceId;
    fn queue_len(&self) -> Result<usize, AccessRefused>;
    fn try_receive(&self) -> Result<Option<OwnedNetworkTxFrame<'resources>>, AccessRefused>;
}

impl<'resources, X: ExecutionContext, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize>
    OwnedNetworkRunner<'resources, X, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
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

    /// Number of complete TX owners waiting in destination queues.
    pub fn tx_queue_len(&self) -> Result<usize, AccessRefused> {
        self.tx.len()
    }

    /// Copy one received Ethernet frame directly into its final Xarxa owner.
    pub fn try_send_rx(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.rx_publisher().try_send(frame)
    }

    /// Publish an already-owned RX packet without copying it again.
    pub fn try_publish_rx(&mut self, packet: PacketBuf) -> Result<(), RxEnqueueError> {
        self.rx_publisher().try_publish(packet)
    }

    /// Claim the next current-lifetime TX owner, dropping stale lifetimes.
    pub fn try_receive_tx(&self) -> Result<Option<OwnedNetworkTxFrame<'resources>>, AccessRefused> {
        self.take_tx(None)
    }

    fn take_tx(
        &self,
        destination: Option<[u8; 6]>,
    ) -> Result<Option<OwnedNetworkTxFrame<'resources>>, AccessRefused> {
        // Bound stale disposal even if another core keeps publishing during
        // teardown. Packet destruction never runs under the metadata lock.
        for _ in 0..TX_QUEUE_DEPTH {
            let Some((queued, credit)) = self.tx.pop(destination)? else {
                return Ok(None);
            };
            let current = self.link.snapshot();
            if current.up && current.epoch == queued.epoch {
                return Ok(Some(OwnedNetworkTxFrame {
                    interface: self.interface,
                    packet: queued.packet,
                    _credit: credit,
                }));
            }
            drop(queued);
            drop(credit);
        }
        Ok(None)
    }

    /// Wait for and claim the next current-lifetime TX owner.
    pub async fn receive_tx(&mut self) -> Result<OwnedNetworkTxFrame<'resources>, AccessRefused> {
        loop {
            if let Some(frame) = self.try_receive_tx()? {
                return Ok(frame);
            }
            poll_fn(|cx| self.tx.poll_publication(cx)).await?;
        }
    }

    /// Wait until some TX publication exists, or return a context refusal.
    ///
    /// This deliberately has no "wait for BA-sized queue" variant: sparse
    /// work must become eligible immediately. Aggregation policy belongs to
    /// the radio scheduler after it claims the owner.
    pub async fn wait_tx_publication(&mut self) -> Result<(), AccessRefused> {
        if self.tx.is_empty()? {
            poll_fn(|cx| self.tx.poll_publication(cx)).await?;
        }
        Ok(())
    }

    /// Wait until the submission frontier reaches `minimum` packets.
    ///
    /// This observes publication events rather than repeatedly polling a
    /// nonempty channel, so a burst collector cannot spin while waiting for
    /// another frame. Callers must still apply their own sparse-traffic
    /// deadline; this method does not require a BA-sized burst.
    pub async fn wait_tx_queue_len_at_least(
        &mut self,
        minimum: usize,
    ) -> Result<(), AccessRefused> {
        while self.tx.len()? < minimum {
            poll_fn(|cx| self.tx.poll_publication(cx)).await?;
        }
        Ok(())
    }
}

impl<'resources, X: ExecutionContext, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize>
    DestinationTxQueues for OwnedNetworkRunner<'resources, X, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    type Frame = OwnedNetworkTxFrame<'resources>;
    type Error = AccessRefused;

    fn next_head_after(
        &self,
        after: Option<[u8; 6]>,
    ) -> Result<Option<([u8; 6], DestinationTxHead)>, Self::Error> {
        self.tx.next_head_after(after)
    }

    fn head_for(&self, destination: [u8; 6]) -> Result<Option<DestinationTxHead>, Self::Error> {
        self.tx.head_for(destination)
    }

    fn try_take_for(&self, destination: [u8; 6]) -> Result<Option<Self::Frame>, Self::Error> {
        self.take_tx(Some(destination))
    }

    fn poll_ready_for(
        &self,
        destination: [u8; 6],
        minimum: usize,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.tx.poll_ready_for(destination, minimum, context)
    }

    fn poll_ready_any(&self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.tx.poll_ready_any(context)
    }
}

impl<'resources, X: ExecutionContext, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize>
    OwnedTxFrameSource<'resources>
    for OwnedNetworkRunner<'resources, X, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    fn interface(&self) -> NetworkInterfaceId {
        OwnedNetworkRunner::interface(self)
    }

    fn queue_len(&self) -> Result<usize, AccessRefused> {
        OwnedNetworkRunner::tx_queue_len(self)
    }

    fn try_receive(&self) -> Result<Option<OwnedNetworkTxFrame<'resources>>, AccessRefused> {
        OwnedNetworkRunner::try_receive_tx(self)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros, clippy::disallowed_methods)]
mod tests;
