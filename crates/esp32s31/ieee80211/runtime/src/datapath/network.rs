//! Role-neutral network capabilities owned by the physical Wi-Fi datapath.

use core::future::Future;

pub use oer_ieee80211_mac::data::EthernetFrameParts;
use oer_network_interface::{LinkState, NetworkInterfaceId, RxEnqueueError};

use super::{SelectedBurstMaterializer, SoftwareTxFrame};

/// Network interface that carries station traffic, alone or beside a SoftAP.
pub const STA_NETWORK_INTERFACE_ID: NetworkInterfaceId = NetworkInterfaceId::new(0);
/// Network interface that carries SoftAP traffic, alone or beside a station.
pub const AP_NETWORK_INTERFACE_ID: NetworkInterfaceId = NetworkInterfaceId::new(1);

/// RX-only network publication capability exposed to one finite DATAPATH service.
/// It cannot observe or claim network-owned TX slots.
pub trait DatapathNetworkRx {
    /// Whether a standalone RX service may wait while holding the radio owner.
    /// Select `DropFrame` when releasing RX capacity can require TX progress
    /// from that same owner. Publication still reports the actual queue error.
    fn backpressure(&self) -> RxBackpressure {
        RxBackpressure::WaitForCapacity
    }

    /// Whether pool exhaustion has a release notification that can resume a
    /// retained frame. A global pool without such a notification must drop
    /// that frame instead of retrying forever or polling an always-ready queue.
    fn pool_exhaustion(&self) -> RxPoolExhaustion {
        RxPoolExhaustion::WaitForRelease
    }
    /// Number of copied RX frames still waiting in the owned network queue.
    fn queue_len(&self) -> usize;

    fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError>;

    fn try_send_parts(&mut self, frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError>;

    /// Reserve one zero-copy publication before the radio rewrites a staged
    /// frame in place. `false` leaves the frame to [`Self::try_send_parts`].
    fn admit_in_place(&mut self) -> bool {
        false
    }

    /// Publish the ready handoff slot `index` under the admission taken by
    /// [`Self::admit_in_place`]. Whatever the result, the slot now belongs to
    /// the network or was released.
    fn publish_in_place(&mut self, index: u8) -> Result<(), RxEnqueueError> {
        let _ = index;
        unreachable!("an in-place publication requires a successful admission")
    }

    /// Return an admission whose frame could not be rewritten in place.
    fn cancel_in_place(&mut self) {}

    /// Poll the next publication credit without allocating a boxed future.
    fn poll_ready(&mut self, context: &mut core::task::Context<'_>) -> core::task::Poll<()>;

    /// Publish one frame, calling `before_publish` once it is admitted and
    /// immediately before publication. Diagnostics use the callback to time
    /// the publication edge. The default observes just before `try_send`; a
    /// publisher that constructs the frame first overrides it to observe
    /// after construction.
    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        before_publish();
        self.try_send(frame)
    }

    /// Frame-parts form of [`Self::try_send_observed`].
    fn try_send_parts_observed(
        &mut self,
        frame: EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        before_publish();
        self.try_send_parts(frame)
    }
}

/// Admission when a finite RX turn cannot obtain an output slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxBackpressure {
    /// Another independently progressing owner can return capacity.
    WaitForCapacity,
    /// Try publication once and account a drop rather than block radio TX.
    DropFrame,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxPoolExhaustion {
    WaitForRelease,
    DropFrame,
}

/// RX publication authority presented to one DATAPATH services graph.
///
/// Standalone roles use only `primary_mut`. Same-channel compositions must
/// select a concrete endpoint by identity after fact-only VIF routing. The
/// trait has no fallback from an unknown identity to the primary endpoint.
pub trait DatapathNetworkRxSet {
    fn primary_mut(&mut self) -> &mut dyn DatapathNetworkRx;

    fn get_mut(&mut self, interface: NetworkInterfaceId) -> Option<&mut dyn DatapathNetworkRx>;

    fn pair_mut(
        &mut self,
        first: NetworkInterfaceId,
        second: NetworkInterfaceId,
    ) -> Option<(&mut dyn DatapathNetworkRx, &mut dyn DatapathNetworkRx)>;

    fn poll_primary_ready(
        &mut self,
        context: &mut core::task::Context<'_>,
    ) -> core::task::Poll<()> {
        self.primary_mut().poll_ready(context)
    }

    fn poll_any_ready(&mut self, context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        self.poll_primary_ready(context)
    }
}

/// Addressed RX publication endpoints owned by one physical DATAPATH.
///
/// A same-channel STA+AP scheduler must select the logical endpoint only
/// after the common RX dispatcher has classified the retained 802.11 owner.
/// Returning `None` for an unknown identity keeps that failure explicit; the
/// caller must account and release the exact frame instead of publishing it
/// through whichever role happens to be active.
pub struct DatapathNetworkRxEndpoints<A, B> {
    first_interface: NetworkInterfaceId,
    first: A,
    second_interface: NetworkInterfaceId,
    second: B,
}

impl<A, B> DatapathNetworkRxEndpoints<A, B> {
    pub fn new(
        first_interface: NetworkInterfaceId,
        first: A,
        second_interface: NetworkInterfaceId,
        second: B,
    ) -> Self {
        assert_ne!(
            first_interface, second_interface,
            "DATAPATH RX endpoints require distinct interface identities"
        );
        Self {
            first_interface,
            first,
            second_interface,
            second,
        }
    }

    pub const fn first_interface(&self) -> NetworkInterfaceId {
        self.first_interface
    }

    pub const fn second_interface(&self) -> NetworkInterfaceId {
        self.second_interface
    }

    pub fn get_mut(&mut self, interface: NetworkInterfaceId) -> Option<&mut dyn DatapathNetworkRx>
    where
        A: DatapathNetworkRx,
        B: DatapathNetworkRx,
    {
        if interface == self.first_interface {
            Some(&mut self.first)
        } else if interface == self.second_interface {
            Some(&mut self.second)
        } else {
            None
        }
    }

    pub fn into_parts(self) -> (A, B) {
        (self.first, self.second)
    }
}

impl<A: DatapathNetworkRx, B: DatapathNetworkRx> DatapathNetworkRxSet
    for DatapathNetworkRxEndpoints<A, B>
{
    fn primary_mut(&mut self) -> &mut dyn DatapathNetworkRx {
        &mut self.first
    }

    fn get_mut(&mut self, interface: NetworkInterfaceId) -> Option<&mut dyn DatapathNetworkRx> {
        DatapathNetworkRxEndpoints::get_mut(self, interface)
    }

    fn pair_mut(
        &mut self,
        first: NetworkInterfaceId,
        second: NetworkInterfaceId,
    ) -> Option<(&mut dyn DatapathNetworkRx, &mut dyn DatapathNetworkRx)> {
        if first == self.first_interface && second == self.second_interface {
            Some((&mut self.first, &mut self.second))
        } else if first == self.second_interface && second == self.first_interface {
            Some((&mut self.second, &mut self.first))
        } else {
            None
        }
    }

    fn poll_any_ready(&mut self, context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        if self.first.poll_ready(context).is_ready() {
            core::task::Poll::Ready(())
        } else {
            self.second.poll_ready(context)
        }
    }
}

/// Radio-side network ownership consumed by [`DatapathRunner`](crate::datapath::DatapathRunner).
///
/// Single-VIF owners expose one RX endpoint. A dual owner selects between
/// permanent STA/AP RX endpoints while retaining the sole tagged TX consumer.
/// Role-specific semantics remain outside this scheduler contract.
pub trait DatapathNetwork {
    type LinkController: DatapathNetworkLink + Copy;
    type RxPublisher: DatapathNetworkRxSet;
    type TxFrame: SoftwareTxFrame;
    type PhysicalTxFrame: crate::datapath::MaterializedTxFrame;
    type TxConsumer<'network>: SelectedBurstMaterializer<
            SoftwareFrame = Self::TxFrame,
            PhysicalFrame = Self::PhysicalTxFrame,
        >
    where
        Self: 'network;

    fn link_controller(&self) -> Self::LinkController;

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher;
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState);
    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize;
    fn try_receive_tx(&self, interface: NetworkInterfaceId) -> Option<Self::TxFrame>;
    fn receive_tx(&self, interface: NetworkInterfaceId)
    -> impl Future<Output = Self::TxFrame> + '_;
    fn tx_consumer(&self, interface: NetworkInterfaceId) -> Self::TxConsumer<'_>;
    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_;
    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_;
    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_;
}

/// Link-only capability which may coexist with the unique Core0 scheduler.
pub trait DatapathNetworkLink {
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState);
}

impl<N> DatapathNetwork for &N
where
    N: DatapathNetwork + ?Sized,
{
    type LinkController = N::LinkController;
    type RxPublisher = N::RxPublisher;
    type TxFrame = N::TxFrame;
    type PhysicalTxFrame = N::PhysicalTxFrame;
    type TxConsumer<'network>
        = N::TxConsumer<'network>
    where
        Self: 'network;

    fn link_controller(&self) -> Self::LinkController {
        N::link_controller(*self)
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        N::rx_publisher(*self, interface)
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        N::set_link_state(*self, interface, state);
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        N::tx_queue_len(*self, interface)
    }

    fn try_receive_tx(&self, interface: NetworkInterfaceId) -> Option<Self::TxFrame> {
        N::try_receive_tx(*self, interface)
    }

    fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> impl Future<Output = Self::TxFrame> + '_ {
        N::receive_tx(*self, interface)
    }

    fn tx_consumer(&self, interface: NetworkInterfaceId) -> Self::TxConsumer<'_> {
        N::tx_consumer(*self, interface)
    }

    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_ {
        N::wait_tx_ready(*self, interface)
    }

    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_ {
        N::wait_tx_queue_len_at_least(*self, interface, minimum)
    }

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        N::wait_tx_publication(*self)
    }
}

impl<N> DatapathNetwork for &mut N
where
    N: DatapathNetwork + ?Sized,
{
    type LinkController = N::LinkController;
    type RxPublisher = N::RxPublisher;
    type TxFrame = N::TxFrame;
    type PhysicalTxFrame = N::PhysicalTxFrame;
    type TxConsumer<'network>
        = N::TxConsumer<'network>
    where
        Self: 'network;

    fn link_controller(&self) -> Self::LinkController {
        N::link_controller(*self)
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        N::rx_publisher(*self, interface)
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        N::set_link_state(*self, interface, state);
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        N::tx_queue_len(*self, interface)
    }

    fn try_receive_tx(&self, interface: NetworkInterfaceId) -> Option<Self::TxFrame> {
        N::try_receive_tx(*self, interface)
    }

    fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> impl Future<Output = Self::TxFrame> + '_ {
        N::receive_tx(*self, interface)
    }

    fn tx_consumer(&self, interface: NetworkInterfaceId) -> Self::TxConsumer<'_> {
        N::tx_consumer(*self, interface)
    }

    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_ {
        N::wait_tx_ready(*self, interface)
    }

    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_ {
        N::wait_tx_queue_len_at_least(*self, interface, minimum)
    }

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        N::wait_tx_publication(*self)
    }
}

#[cfg(test)]
mod tests;
