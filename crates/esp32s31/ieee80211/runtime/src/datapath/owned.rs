//! Owned-packet network glue for the pinned Git Embassy/Xarxa stack.
//!
//! Everything in this module exists only with `owned-network`. It binds the
//! owned adapter's RX publishers, link controllers and software TX frontier to
//! the role-neutral datapath network contracts and to pinned SRAM
//! materialization. The released-interface and Xarxa integrations implement the
//! same contracts in their own bridge crates.

use core::future::Future;

use embassy_futures::select::select;
use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_embassy_net_owned::{
    ExternalRxAdmission, ExternalRxOrigin, OwnedLinkController, OwnedNetworkRunner,
    OwnedNetworkTxFrame, OwnedRxPublisher, OwnedTxFrameSource,
};
use oer_esp32s31_ieee80211_mac::rx::pool::{
    VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT,
};
use oer_ieee80211_datapath::MaterializationOwnershipSnapshot;
use oer_ieee80211_datapath::{MaterializedPairResult, SelectedBurstMaterializer};
use oer_network_interface::{LinkState, NetworkInterfaceId, RxEnqueueError};

use super::{
    PinnedTxConsumer, PinnedTxFrame, PinnedTxInterfaceConsumer,
    network::{
        DatapathNetwork, DatapathNetworkLink, DatapathNetworkRx, DatapathNetworkRxSet,
        EthernetFrameParts, STA_NETWORK_INTERFACE_ID,
    },
};

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize> DatapathNetworkRxSet
    for OwnedRxPublisher<'_, M, RX_QUEUE_DEPTH>
{
    fn primary_mut(&mut self) -> &mut dyn DatapathNetworkRx {
        self
    }

    fn get_mut(&mut self, _interface: NetworkInterfaceId) -> Option<&mut dyn DatapathNetworkRx> {
        None
    }

    fn pair_mut(
        &mut self,
        _first: NetworkInterfaceId,
        _second: NetworkInterfaceId,
    ) -> Option<(&mut dyn DatapathNetworkRx, &mut dyn DatapathNetworkRx)> {
        None
    }
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize> DatapathNetworkRx
    for OwnedRxPublisher<'_, M, RX_QUEUE_DEPTH>
{
    fn queue_len(&self) -> usize {
        OwnedRxPublisher::queue_len(self)
    }

    fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        OwnedRxPublisher::try_send(self, frame)
    }

    fn try_send_parts(&mut self, frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError> {
        OwnedRxPublisher::try_send_parts(
            self,
            frame.destination,
            frame.source,
            frame.ether_type,
            frame.payload,
        )
    }

    fn poll_ready(&mut self, context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        OwnedRxPublisher::poll_ready(self, context)
    }

    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        before_publish();
        OwnedRxPublisher::try_send(self, frame)
    }

    fn try_send_parts_observed(
        &mut self,
        frame: EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        before_publish();
        OwnedRxPublisher::try_send_parts(
            self,
            frame.destination,
            frame.source,
            frame.ether_type,
            frame.payload,
        )
    }
}

/// Zero-copy origin of the physical RX staging pool: one per handoff pool,
/// shared by every endpoint because each held slot is a physical DMA credit.
pub type RxZeroCopyOrigin =
    ExternalRxOrigin<VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT>;

/// Owned RX publisher that adopts staged DMA buffers in place when the
/// zero-copy origin admits them, and copies otherwise.
pub struct ZeroCopyRxPublisher<'resources, M: RawMutex, const RX_QUEUE_DEPTH: usize> {
    inner: OwnedRxPublisher<'resources, M, RX_QUEUE_DEPTH>,
    origin: &'static RxZeroCopyOrigin,
    admission:
        Option<ExternalRxAdmission<VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT>>,
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize> DatapathNetworkRxSet
    for ZeroCopyRxPublisher<'_, M, RX_QUEUE_DEPTH>
{
    fn primary_mut(&mut self) -> &mut dyn DatapathNetworkRx {
        self
    }

    fn get_mut(&mut self, _interface: NetworkInterfaceId) -> Option<&mut dyn DatapathNetworkRx> {
        None
    }

    fn pair_mut(
        &mut self,
        _first: NetworkInterfaceId,
        _second: NetworkInterfaceId,
    ) -> Option<(&mut dyn DatapathNetworkRx, &mut dyn DatapathNetworkRx)> {
        None
    }
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize> DatapathNetworkRx
    for ZeroCopyRxPublisher<'_, M, RX_QUEUE_DEPTH>
{
    fn queue_len(&self) -> usize {
        self.inner.queue_len()
    }

    fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        DatapathNetworkRx::try_send(&mut self.inner, frame)
    }

    fn try_send_parts(&mut self, frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError> {
        DatapathNetworkRx::try_send_parts(&mut self.inner, frame)
    }

    fn admit_in_place(&mut self) -> bool {
        debug_assert!(self.admission.is_none());
        self.admission = self.inner.try_admit_external(self.origin).ok();
        self.admission.is_some()
    }

    fn publish_in_place(&mut self, index: u8) -> Result<(), RxEnqueueError> {
        let admission = self
            .admission
            .take()
            .expect("an in-place publication requires a successful admission");
        self.inner.publish_external(admission, index)
    }

    fn cancel_in_place(&mut self) {
        self.admission = None;
    }

    fn poll_ready(&mut self, context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        self.inner.poll_ready(context)
    }

    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        DatapathNetworkRx::try_send_observed(&mut self.inner, frame, before_publish)
    }

    fn try_send_parts_observed(
        &mut self,
        frame: EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        DatapathNetworkRx::try_send_parts_observed(&mut self.inner, frame, before_publish)
    }
}

impl<M: RawMutex> DatapathNetworkLink for OwnedLinkController<'_, M> {
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        assert_eq!(
            interface,
            self.interface(),
            "owned link controller cannot change another interface"
        );
        self.set_link_up(matches!(state, LinkState::Up));
    }
}

/// Link-only authority for two permanent owned network endpoints.
///
/// The physical DATAPATH retains this pair across STA, AP and same-channel
/// STA+AP epochs. A role transition can therefore change only its addressed
/// logical interface and cannot accidentally publish link state through the
/// other endpoint.
pub struct OwnedNetworkLinkControllers<'resources, M: RawMutex> {
    first: OwnedLinkController<'resources, M>,
    second: OwnedLinkController<'resources, M>,
}

impl<M: RawMutex> Clone for OwnedNetworkLinkControllers<'_, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex> Copy for OwnedNetworkLinkControllers<'_, M> {}

impl<'resources, M: RawMutex> OwnedNetworkLinkControllers<'resources, M> {
    fn new(
        first: OwnedLinkController<'resources, M>,
        second: OwnedLinkController<'resources, M>,
    ) -> Self {
        assert_ne!(
            first.interface(),
            second.interface(),
            "dual owned endpoints require distinct interface identities"
        );
        Self { first, second }
    }
}

impl<M: RawMutex> DatapathNetworkLink for OwnedNetworkLinkControllers<'_, M> {
    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        let controller = if interface == self.first.interface() {
            self.first
        } else {
            assert_eq!(
                interface,
                self.second.interface(),
                "link interface does not belong to this dual owned owner"
            );
            self.second
        };
        controller.set_link_up(matches!(state, LinkState::Up));
    }
}

/// One owned Xarxa endpoint backed by a separate physical SRAM TX horizon.
///
/// `NETWORK_TX_DEPTH` bounds complete packet owners waiting for radio
/// selection. `TX_QUEUE_DEPTH` bounds DMA-capable execution storage. Keeping
/// the dimensions independent prevents software backlog policy from silently
/// consuming additional physical radio credits as peer count grows.
pub struct OwnedDatapathNetwork<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const NETWORK_TX_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    network: OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>,
    physical: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
}

/// Two owned Xarxa endpoints sharing one fixed physical SRAM TX horizon.
///
/// Each logical interface has an independent bounded software-owner queue.
/// The sole Core0 DATAPATH chooses an interface before claiming its next
/// owner, then promotes only the selected frame or burst through `physical`.
/// Associated-peer count therefore changes software queue metadata/backlog,
/// never the number of DMA-capable SRAM slots.
pub struct DualOwnedDatapathNetwork<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const NETWORK_TX_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    first: OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>,
    second: OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>,
    physical: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    rx_zero_copy: &'static RxZeroCopyOrigin,
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const NETWORK_TX_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
    DualOwnedDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        NETWORK_TX_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    pub fn new(
        first: OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>,
        second: OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>,
        physical: PinnedTxConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
        rx_zero_copy: &'static RxZeroCopyOrigin,
    ) -> Self {
        assert_ne!(
            first.interface(),
            second.interface(),
            "dual owned endpoints require distinct interface identities"
        );
        Self {
            first,
            second,
            physical,
            rx_zero_copy,
        }
    }

    fn endpoint(
        &self,
        interface: NetworkInterfaceId,
    ) -> &OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH> {
        if interface == self.first.interface() {
            &self.first
        } else {
            assert_eq!(
                interface,
                self.second.interface(),
                "network interface does not belong to this dual owned owner"
            );
            &self.second
        }
    }
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const NETWORK_TX_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
    OwnedDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        NETWORK_TX_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    pub const fn new(
        network: OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>,
        physical: PinnedTxConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            TX_QUEUE_DEPTH,
        >,
    ) -> Self {
        Self { network, physical }
    }

    pub fn into_parts(
        self,
    ) -> (
        OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>,
        PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    ) {
        (self.network, self.physical)
    }

    pub const fn interface(&self) -> NetworkInterfaceId {
        self.network.interface()
    }

    fn assert_interface(&self, interface: NetworkInterfaceId) {
        assert_eq!(
            interface,
            self.interface(),
            "single owned network cannot access another interface"
        );
    }
}

impl<
    'resources,
    M: RawMutex + 'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const NETWORK_TX_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> DatapathNetwork
    for OwnedDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        NETWORK_TX_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    type LinkController = OwnedLinkController<'resources, M>;
    type RxPublisher = OwnedRxPublisher<'resources, M, RX_QUEUE_DEPTH>;
    type TxFrame = OwnedNetworkTxFrame<'resources, M>;
    type PhysicalTxFrame =
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;
    type TxConsumer<'network>
        = DatapathTxConsumer<
        'network,
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        TX_QUEUE_DEPTH,
    >
    where
        Self: 'network;

    fn link_controller(&self) -> Self::LinkController {
        self.network.link_controller()
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        self.assert_interface(interface);
        self.network.rx_publisher()
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        self.link_controller().set_link_state(interface, state);
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        self.assert_interface(interface);
        self.network.tx_queue_len()
    }

    fn try_receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<OwnedNetworkTxFrame<'resources, M>> {
        self.assert_interface(interface);
        self.network.try_receive_tx()
    }

    async fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> OwnedNetworkTxFrame<'resources, M> {
        self.assert_interface(interface);
        self.network.receive_tx().await
    }

    fn tx_consumer(
        &self,
        interface: NetworkInterfaceId,
    ) -> DatapathTxConsumer<'_, 'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        self.assert_interface(interface);
        DatapathTxConsumer::new(&self.network, self.physical.for_interface(interface))
    }

    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_ {
        self.assert_interface(interface);
        self.network.wait_tx_queue_len_at_least(1)
    }

    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_ {
        self.assert_interface(interface);
        self.network.wait_tx_queue_len_at_least(minimum)
    }

    fn wait_tx_publication(&self) -> impl Future<Output = ()> + '_ {
        self.network.wait_tx_publication()
    }
}

impl<
    'resources,
    M: RawMutex + 'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const NETWORK_TX_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> oer_ieee80211_runtime::station_network::StationNetworkLink
    for DualOwnedDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        NETWORK_TX_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    fn publish_link_up(&self) {
        self.set_link_state(STA_NETWORK_INTERFACE_ID, LinkState::Up);
    }
}

impl<
    'resources,
    M: RawMutex + 'resources,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const NETWORK_TX_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> DatapathNetwork
    for DualOwnedDatapathNetwork<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        NETWORK_TX_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    type LinkController = OwnedNetworkLinkControllers<'resources, M>;
    type RxPublisher = ZeroCopyRxPublisher<'resources, M, RX_QUEUE_DEPTH>;
    type TxFrame = OwnedNetworkTxFrame<'resources, M>;
    type PhysicalTxFrame =
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;
    type TxConsumer<'network>
        = DatapathTxConsumer<
        'network,
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        TX_QUEUE_DEPTH,
    >
    where
        Self: 'network;

    fn link_controller(&self) -> Self::LinkController {
        OwnedNetworkLinkControllers::new(
            self.first.link_controller(),
            self.second.link_controller(),
        )
    }

    fn rx_publisher(&self, interface: NetworkInterfaceId) -> Self::RxPublisher {
        ZeroCopyRxPublisher {
            inner: self.endpoint(interface).rx_publisher(),
            origin: self.rx_zero_copy,
            admission: None,
        }
    }

    fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        self.link_controller().set_link_state(interface, state);
    }

    fn tx_queue_len(&self, interface: NetworkInterfaceId) -> usize {
        self.endpoint(interface).tx_queue_len()
    }

    fn try_receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<OwnedNetworkTxFrame<'resources, M>> {
        self.endpoint(interface).try_receive_tx()
    }

    async fn receive_tx(
        &self,
        interface: NetworkInterfaceId,
    ) -> OwnedNetworkTxFrame<'resources, M> {
        self.endpoint(interface).receive_tx().await
    }

    fn tx_consumer(
        &self,
        interface: NetworkInterfaceId,
    ) -> DatapathTxConsumer<'_, 'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        DatapathTxConsumer::new(
            self.endpoint(interface),
            self.physical.for_interface(interface),
        )
    }

    fn wait_tx_ready(&self, interface: NetworkInterfaceId) -> impl Future<Output = ()> + '_ {
        self.endpoint(interface).wait_tx_queue_len_at_least(1)
    }

    fn wait_tx_queue_len_at_least(
        &self,
        interface: NetworkInterfaceId,
        minimum: usize,
    ) -> impl Future<Output = ()> + '_ {
        self.endpoint(interface).wait_tx_queue_len_at_least(minimum)
    }

    async fn wait_tx_publication(&self) {
        let _ = select(
            self.first.wait_tx_publication(),
            self.second.wait_tx_publication(),
        )
        .await;
    }
}

impl<
    'source,
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> SelectedBurstMaterializer
    for DatapathTxConsumer<'source, 'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    type SoftwareFrame = OwnedNetworkTxFrame<'resources, M>;
    type PhysicalFrame =
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;

    fn interface(&self) -> NetworkInterfaceId {
        DatapathTxConsumer::interface(self)
    }

    fn queue_len(&self) -> usize {
        DatapathTxConsumer::queue_len(self)
    }

    fn try_take(&self) -> Option<Self::SoftwareFrame> {
        DatapathTxConsumer::try_receive(self)
    }

    fn destination_queues(
        &self,
    ) -> Option<&dyn oer_ieee80211_datapath::DestinationTxQueues<Frame = Self::SoftwareFrame>> {
        Some(self.source)
    }

    fn try_materialize(
        &self,
        frame: Self::SoftwareFrame,
    ) -> Result<Self::PhysicalFrame, Self::SoftwareFrame> {
        DatapathTxConsumer::try_promote(self, frame)
    }

    fn try_materialize_next(&self) -> Option<Self::PhysicalFrame> {
        DatapathTxConsumer::try_receive_direct(self)
    }

    fn materialization_capacity(&self) -> usize {
        DatapathTxConsumer::promotion_capacity(self)
    }

    fn ownership_snapshot(&self) -> MaterializationOwnershipSnapshot {
        DatapathTxConsumer::ownership_snapshot(self)
    }

    fn try_materialize_batch<const BATCH: usize>(
        &self,
        sources: &mut [Option<Self::SoftwareFrame>; BATCH],
        destinations: &mut [Option<Self::PhysicalFrame>; BATCH],
    ) -> bool {
        DatapathTxConsumer::try_promote_batch(self, sources, destinations)
    }
}

/// Radio-side composition of an owned software frontier and physical SRAM.
pub struct DatapathTxConsumer<
    'source,
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    source: &'source dyn OwnedTxFrameSource<'resources, M>,
    physical:
        PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Clone
    for DatapathTxConsumer<'_, '_, M, F, H, T, Q>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Copy
    for DatapathTxConsumer<'_, '_, M, F, H, T, Q>
{
}

impl<
    'source,
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> DatapathTxConsumer<'source, 'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub fn new(
        source: &'source dyn OwnedTxFrameSource<'resources, M>,
        physical: PinnedTxInterfaceConsumer<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
    ) -> Self {
        assert_eq!(source.interface(), physical.interface());
        Self { source, physical }
    }

    pub fn interface(&self) -> NetworkInterfaceId {
        self.source.interface()
    }

    pub fn queue_len(&self) -> usize {
        self.source.queue_len()
    }

    pub fn try_receive(&self) -> Option<OwnedNetworkTxFrame<'resources, M>> {
        self.source.try_receive()
    }

    pub fn try_promote(
        &self,
        frame: OwnedNetworkTxFrame<'resources, M>,
    ) -> Result<
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        OwnedNetworkTxFrame<'resources, M>,
    > {
        self.physical.try_materialize(frame)
    }

    pub fn try_receive_direct(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>> {
        self.physical
            .try_materialize_from(|| self.source.try_receive())
    }

    pub fn promotion_capacity(&self) -> usize {
        self.physical.promotion_capacity()
    }

    pub fn try_promote_batch<const BATCH: usize>(
        &self,
        sources: &mut [Option<OwnedNetworkTxFrame<'resources, M>>; BATCH],
        destinations: &mut [Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>>;
                 BATCH],
    ) -> bool {
        self.physical.try_materialize_batch(sources, destinations)
    }

    pub fn try_promote_pair(
        &self,
        first: OwnedNetworkTxFrame<'resources, M>,
        second: OwnedNetworkTxFrame<'resources, M>,
    ) -> MaterializedPairResult<
        OwnedNetworkTxFrame<'resources, M>,
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    > {
        let mut sources = [Some(first), Some(second)];
        let mut destinations = [None, None];
        if !self.try_promote_batch(&mut sources, &mut destinations) {
            return Err((
                sources[0].take().expect("failed pair retains first owner"),
                sources[1].take().expect("failed pair retains second owner"),
            ));
        }
        Ok((
            destinations[0]
                .take()
                .expect("successful pair publishes first owner"),
            destinations[1]
                .take()
                .expect("successful pair publishes second owner"),
        ))
    }

    pub fn ownership_snapshot(&self) -> MaterializationOwnershipSnapshot {
        self.physical.ownership_snapshot()
    }
}
