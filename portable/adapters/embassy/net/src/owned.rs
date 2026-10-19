//! Owned packet boundary used by the Xarxa/Embassy cutover.
//!
//! This module contains no Wi-Fi scheduler. It moves complete [`PacketBuf`]
//! owners between the network and radio execution domains through bounded
//! queues. TX is indexed by Ethernet destination before publication. Radio
//! policy selects a destination before claiming owners and validates peer/TID
//! eligibility before SRAM promotion.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use core::task::{Context, Poll, Waker};

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender, TrySendError};
use embassy_sync::once_lock::OnceLock;
use embassy_sync::signal::Signal;
use embassy_sync::waitqueue::GenericAtomicWaker;
use xarxa_driver::{
    Capabilities, ChecksumCapabilities, Driver, HardwareAddress, LinkState as DriverLinkState,
    NotSupported, PacketBuf, PacketBufAllocator, PacketPoolWaiter,
};

use oer_ieee80211_datapath::{DestinationTxHead, DestinationTxQueues};

mod external_rx;
mod tx_budget;
mod tx_queue;
pub use external_rx::{
    ExternalRxAdmission, ExternalRxCounters, ExternalRxOrigin, ExternalRxRefusal,
};
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
struct OwnedLinkState<M: RawMutex> {
    state: AtomicU32,
    network_waker: GenericAtomicWaker<M>,
    radio_waker: GenericAtomicWaker<M>,
}

impl<M: RawMutex> OwnedLinkState<M> {
    const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
            network_waker: GenericAtomicWaker::new(M::INIT),
            radio_waker: GenericAtomicWaker::new(M::INIT),
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

/// Static bounded queues for one permanent logical network endpoint.
///
/// Packet bytes do not live in this value. RX owners come from the allocator
/// supplied to [`split`](Self::split); TX owners retain the general Xarxa pool
/// selected by the application. `TX_QUEUE_DEPTH` bounds all admitted software
/// owners, including frames retained by the radio after dequeue. Physical DMA
/// storage has its own lifetime and budget.
pub struct OwnedEndpointResources<
    M: RawMutex,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    rx: Channel<M, QueuedPacket, RX_QUEUE_DEPTH>,
    tx: TxQueue<M, TX_QUEUE_DEPTH>,
    tx_published: Signal<M, ()>,
    rx_waiter: OnceLock<PacketPoolWaiter>,
    link: OwnedLinkState<M>,
    split: AtomicBool,
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize>
    OwnedEndpointResources<M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    /// Create an inactive, empty endpoint.
    pub const fn new() -> Self {
        Self {
            rx: Channel::new(),
            tx: TxQueue::new(),
            tx_published: Signal::new(),
            rx_waiter: OnceLock::new(),
            link: OwnedLinkState::new(),
            split: AtomicBool::new(false),
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
    ) -> (
        OwnedNetworkDevice<'_, M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>,
        OwnedNetworkRunner<'_, M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>,
    ) {
        assert!(RX_QUEUE_DEPTH != 0, "owned RX queue must not be empty");
        assert!(TX_QUEUE_DEPTH != 0, "owned TX queue must not be empty");
        assert!(
            !self.split.swap(true, Ordering::AcqRel),
            "owned endpoint resources may only be split once"
        );
        let resources: &Self = self;
        let rx_waiter = rx_allocator
            .try_claim_waiter()
            .expect("an owned RX pool may have only one asynchronous radio waiter");
        resources
            .rx_waiter
            .init(rx_waiter)
            .unwrap_or_else(|_| unreachable!("owned endpoint initializes its RX waiter once"));
        let rx_waiter = resources
            .rx_waiter
            .try_get()
            .expect("owned RX waiter was initialized");
        (
            OwnedNetworkDevice {
                hardware_address,
                rx: resources.rx.receiver(),
                tx: &resources.tx,
                tx_published: &resources.tx_published,
                link: &resources.link,
                checksum: ChecksumCapabilities::default(),
            },
            OwnedNetworkRunner {
                interface,
                rx: resources.rx.sender(),
                tx: &resources.tx,
                tx_published: &resources.tx_published,
                link: &resources.link,
                rx_allocator,
                rx_waiter,
            },
        )
    }
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize> Default
    for OwnedEndpointResources<M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Xarxa/Embassy side of one owned packet endpoint.
pub struct OwnedNetworkDevice<
    'resources,
    M: RawMutex,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    hardware_address: [u8; 6],
    rx: Receiver<'resources, M, QueuedPacket, RX_QUEUE_DEPTH>,
    tx: &'resources TxQueue<M, TX_QUEUE_DEPTH>,
    tx_published: &'resources Signal<M, ()>,
    link: &'resources OwnedLinkState<M>,
    checksum: ChecksumCapabilities,
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize>
    OwnedNetworkDevice<'_, M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
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
            let queued = self.rx.try_receive().ok()?;
            let current = self.link.snapshot();
            if current.up && current.epoch == queued.epoch {
                return Some(queued.packet);
            }
            // A stale owner returns to its originating RX pool here.
        }
    }

    /// Whether the shared software TX budget can admit one packet now.
    pub fn can_transmit(&self) -> bool {
        self.link.snapshot().up && !self.tx.is_full()
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
            Ok(()) => {
                self.tx_published.signal(());
                Ok(())
            }
            Err(queued) => Err(queued.packet),
        }
    }
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize> Driver
    for OwnedNetworkDevice<'_, M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
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
pub struct OwnedNetworkTxFrame<'resources, M: RawMutex> {
    interface: NetworkInterfaceId,
    packet: PacketBuf,
    // Field order is intentional: return the packet to its pool before waking
    // a producer which can immediately spend this software admission credit.
    _credit: TxCredit<'resources, M>,
}

/// Copyable RX-only capability for a physical datapath service.
///
/// The capability can allocate only from this endpoint's dedicated RX pool;
/// it cannot observe or claim network-originated TX owners.
#[derive(Clone, Copy)]
pub struct OwnedRxPublisher<'resources, M: RawMutex, const RX_QUEUE_DEPTH: usize> {
    rx: Sender<'resources, M, QueuedPacket, RX_QUEUE_DEPTH>,
    link: &'resources OwnedLinkState<M>,
    rx_allocator: PacketBufAllocator,
    rx_waiter: &'resources PacketPoolWaiter,
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize> OwnedRxPublisher<'_, M, RX_QUEUE_DEPTH> {
    /// Number of complete RX owners waiting for Xarxa.
    pub fn queue_len(&self) -> usize {
        self.rx.len()
    }

    /// Poll until one bounded RX queue entry can be published.
    pub fn poll_ready(&self, context: &mut Context<'_>) -> Poll<()> {
        self.link.register_radio_waker(context.waker());
        if !self.link.snapshot().up {
            return Poll::Pending;
        }
        if self.rx.poll_ready_to_send(context).is_pending() {
            return Poll::Pending;
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
    pub fn try_send(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
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
        packet.copy_from_slice(frame);
        self.try_publish(packet)
    }

    /// Build Ethernet-II header and payload directly in the final RX owner.
    pub fn try_send_parts(
        &self,
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
        packet[..6].copy_from_slice(&destination);
        packet[6..12].copy_from_slice(&source);
        packet[12..ETHERNET_HEADER_LEN].copy_from_slice(&ether_type.to_be_bytes());
        packet[ETHERNET_HEADER_LEN..].copy_from_slice(payload);
        self.try_publish(packet)
    }

    /// Publish an already-owned RX packet without copying it again.
    pub fn try_publish(&self, packet: PacketBuf) -> Result<(), RxEnqueueError> {
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
            Err(TrySendError::Full(_)) => Err(RxEnqueueError::QueueFull),
        }
    }
}

impl<M: RawMutex> OwnedNetworkTxFrame<'_, M> {
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

/// Link-state capability which can coexist with packet publication handles.
pub struct OwnedLinkController<'resources, M: RawMutex> {
    interface: NetworkInterfaceId,
    link: &'resources OwnedLinkState<M>,
}

impl<M: RawMutex> Clone for OwnedLinkController<'_, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex> Copy for OwnedLinkController<'_, M> {}

impl<M: RawMutex> OwnedLinkController<'_, M> {
    /// Permanent logical interface controlled by this capability.
    pub const fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    /// Publish the role's link level. A Down -> Up edge creates a new epoch.
    pub fn set_link_up(&self, up: bool) {
        self.link.set(up);
    }
}

/// Sole radio-side owner of one network endpoint.
pub struct OwnedNetworkRunner<
    'resources,
    M: RawMutex,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    interface: NetworkInterfaceId,
    rx: Sender<'resources, M, QueuedPacket, RX_QUEUE_DEPTH>,
    tx: &'resources TxQueue<M, TX_QUEUE_DEPTH>,
    tx_published: &'resources Signal<M, ()>,
    link: &'resources OwnedLinkState<M>,
    rx_allocator: PacketBufAllocator,
    rx_waiter: &'resources PacketPoolWaiter,
}

/// Object-safe radio-side view of one bounded owned TX frontier.
///
/// The trait deliberately exposes no physical memory or scheduler policy. It
/// lets a radio adapter compose the software owner queue with its private SRAM
/// allocator without depending on the queue's compile-time depth.
pub trait OwnedTxFrameSource<'resources, M: RawMutex + 'resources>:
    DestinationTxQueues<Frame = OwnedNetworkTxFrame<'resources, M>>
{
    fn interface(&self) -> NetworkInterfaceId;
    fn queue_len(&self) -> usize;
    fn try_receive(&self) -> Option<OwnedNetworkTxFrame<'resources, M>>;
}

impl<'resources, M: RawMutex, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize>
    OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    /// A copyable link-only capability for role lifecycle code.
    pub const fn link_controller(&self) -> OwnedLinkController<'resources, M> {
        OwnedLinkController {
            interface: self.interface,
            link: self.link,
        }
    }

    /// Permanent logical interface represented by this endpoint.
    pub const fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    /// RX-only publication capability for the physical datapath.
    pub const fn rx_publisher(&self) -> OwnedRxPublisher<'resources, M, RX_QUEUE_DEPTH> {
        OwnedRxPublisher {
            rx: self.rx,
            link: self.link,
            rx_allocator: self.rx_allocator,
            rx_waiter: self.rx_waiter,
        }
    }

    /// Number of complete TX owners waiting in destination queues.
    pub fn tx_queue_len(&self) -> usize {
        self.tx.len()
    }

    /// Copy one received Ethernet frame directly into its final Xarxa owner.
    pub fn try_send_rx(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.rx_publisher().try_send(frame)
    }

    /// Publish an already-owned RX packet without copying it again.
    pub fn try_publish_rx(&self, packet: PacketBuf) -> Result<(), RxEnqueueError> {
        self.rx_publisher().try_publish(packet)
    }

    /// Claim the next current-lifetime TX owner, dropping stale lifetimes.
    pub fn try_receive_tx(&self) -> Option<OwnedNetworkTxFrame<'resources, M>> {
        self.take_tx(None)
    }

    fn take_tx(&self, destination: Option<[u8; 6]>) -> Option<OwnedNetworkTxFrame<'resources, M>> {
        // Bound stale disposal even if another core keeps publishing during
        // teardown. Packet destruction never runs under the metadata lock.
        for _ in 0..TX_QUEUE_DEPTH {
            let (queued, credit) = self.tx.pop(destination)?;
            let current = self.link.snapshot();
            if current.up && current.epoch == queued.epoch {
                return Some(OwnedNetworkTxFrame {
                    interface: self.interface,
                    packet: queued.packet,
                    _credit: credit,
                });
            }
            drop(queued);
            drop(credit);
        }
        None
    }

    /// Wait for and claim the next current-lifetime TX owner.
    pub async fn receive_tx(&self) -> OwnedNetworkTxFrame<'resources, M> {
        loop {
            if let Some(frame) = self.try_receive_tx() {
                return frame;
            }
            self.tx_published.wait().await;
        }
    }

    /// Wait until some TX publication exists.
    ///
    /// This deliberately has no "wait for BA-sized queue" variant: sparse
    /// work must become eligible immediately. Aggregation policy belongs to
    /// the radio scheduler after it claims the owner.
    pub async fn wait_tx_publication(&self) {
        if self.tx.is_empty() {
            self.tx_published.wait().await;
        }
    }

    /// Wait until the submission frontier reaches `minimum` packets.
    ///
    /// This observes publication events rather than repeatedly polling a
    /// nonempty channel, so a burst collector cannot spin while waiting for
    /// another frame. Callers must still apply their own sparse-traffic
    /// deadline; this method does not require a BA-sized burst.
    pub async fn wait_tx_queue_len_at_least(&self, minimum: usize) {
        while self.tx.len() < minimum {
            self.tx_published.wait().await;
        }
    }
}

impl<'resources, M: RawMutex, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize>
    DestinationTxQueues for OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    type Frame = OwnedNetworkTxFrame<'resources, M>;

    fn next_head_after(&self, after: Option<[u8; 6]>) -> Option<([u8; 6], DestinationTxHead)> {
        self.tx.next_head_after(after)
    }

    fn head_for(&self, destination: [u8; 6]) -> Option<DestinationTxHead> {
        self.tx.head_for(destination)
    }

    fn try_take_for(&self, destination: [u8; 6]) -> Option<Self::Frame> {
        self.take_tx(Some(destination))
    }

    fn poll_ready_for(
        &self,
        destination: [u8; 6],
        minimum: usize,
        context: &mut Context<'_>,
    ) -> Poll<()> {
        self.tx.poll_ready_for(destination, minimum, context)
    }

    fn poll_ready_any(&self, context: &mut Context<'_>) -> Poll<()> {
        self.tx.poll_ready_any(context)
    }
}

impl<'resources, M: RawMutex, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize>
    OwnedTxFrameSource<'resources, M>
    for OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    fn interface(&self) -> NetworkInterfaceId {
        OwnedNetworkRunner::interface(self)
    }

    fn queue_len(&self) -> usize {
        OwnedNetworkRunner::tx_queue_len(self)
    }

    fn try_receive(&self) -> Option<OwnedNetworkTxFrame<'resources, M>> {
        OwnedNetworkRunner::try_receive_tx(self)
    }
}

#[cfg(test)]
mod tests;
