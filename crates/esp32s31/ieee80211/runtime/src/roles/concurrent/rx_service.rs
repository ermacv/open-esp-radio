#![expect(
    clippy::manual_async_fn,
    reason = "the shared RX producer keeps its borrowed Future service contract explicit"
)]

//! Finite protocol service over the sole same-channel STA+AP RX producer.

use core::future::Future;

use crate::datapath::{
    DatapathRxProgress, DatapathRxServiceContext, DatapathRxWorkCounters,
    network::DatapathNetworkRx,
    paired::{DatapathPairRole, DatapathPairedRxProgress, DatapathPairedRxService},
    rx::{dma::StagedRxEpoch, staging::StagedRxFrame, turn::FusedRxTurn},
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_ieee80211_mac::rx::{
    RxDma, RxError,
    pool::{RxStageTransactionError, VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT},
};

use oer_ieee80211_mac::vif::StaApRxRoute;

use oer_time::Instant;

use super::{RoutedRxDisposition, StaApRxConsumer, StaApRxTurn};

/// Protocol leases consumed between physical-TX completion checks.
const ACTIVE_TX_PROTOCOL_QUANTUM_FRAMES: usize = 4;

/// Connected-station protocol authority accepted by the common RX owner.
///
/// The role receives only an already-classified staging lease and its own
/// addressed network endpoint. It cannot inspect the AP endpoint or reclaim
/// DMA descriptors.
pub trait StaApStationRxRole<
    'pool,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
>
{
    type Dispatch;
    type Error: 'static;

    fn publish_pending_rx(
        &mut self,
        network: &mut dyn DatapathNetworkRx,
    ) -> Result<DatapathRxProgress, Self::Error>;

    fn service_station_rx<'a>(
        &'a mut self,
        frame: StagedRxFrame<'pool, CAPACITY, SLOTS>,
        network: &'a mut dyn DatapathNetworkRx,
    ) -> impl Future<Output = Result<Self::Dispatch, Self::Error>> + 'a
    where
        'pool: 'a;

    fn has_pending_rx(&self) -> bool;
}

/// SoftAP protocol authority accepted by the common RX owner.
///
/// AP publication can retain a decoded Ethernet batch independently of the
/// staging lease. The explicit publication edge lets the common scheduler
/// stop at network backpressure without scanning past the ordered head.
pub trait StaApAccessPointRxRole<
    'pool,
    H,
    PhysicalTx,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
>
{
    type Error: 'static;

    fn publish_pending_rx(
        &mut self,
        physical_tx: &mut PhysicalTx,
        network: &mut dyn DatapathNetworkRx,
        now: Instant,
    ) -> Result<DatapathRxProgress, Self::Error>;

    fn service_access_point_rx(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut PhysicalTx,
        frame: StagedRxFrame<'pool, CAPACITY, SLOTS>,
        now: Instant,
    ) -> Result<RoutedRxDisposition<StagedRxFrame<'pool, CAPACITY, SLOTS>>, Self::Error>;

    /// Consume protected AP data without borrowing the physical TX or MMIO
    /// owners. Any resulting hardware/control request remains value-only in
    /// the role mailbox until the active transaction reaches a terminal edge.
    fn service_access_point_rx_during_tx(
        &mut self,
        frame: StagedRxFrame<'pool, CAPACITY, SLOTS>,
        now: Instant,
    ) -> Result<RoutedRxDisposition<StagedRxFrame<'pool, CAPACITY, SLOTS>>, Self::Error>;

    fn has_pending_rx(&self, now: Instant) -> bool;

    /// True only while this AP role owns a hardware TX transaction created by
    /// its RX protocol step.
    fn tx_pending(&self) -> bool;
}

/// Origin-preserving failure from one common RX service turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApRxServiceError<StationError, AccessPointError> {
    Dma(RxStageTransactionError),
    Station(StationError),
    AccessPoint(AccessPointError),
}

/// Fact-only outcomes observed at the common STA+AP RX demultiplexer.
///
/// The vendor retains an interface-context pointer on each packet and routes
/// it by comparing that pointer with the active STA context. The open driver
/// has no reviewed equivalent of that private pointer, so it classifies the
/// public IEEE 802.11 header instead. These counters make that replacement
/// boundary observable without assigning protocol meaning to rejected data.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StaApRxRouteReport {
    pub station: u64,
    pub access_point: u64,
    pub foreign: u64,
    pub ambiguous: u64,
    pub malformed: u64,
    pub hardware_error: u64,
}

impl StaApRxRouteReport {
    pub const fn new() -> Self {
        Self {
            station: 0,
            access_point: 0,
            foreign: 0,
            ambiguous: 0,
            malformed: 0,
            hardware_error: 0,
        }
    }

    pub const fn total(self) -> u64 {
        self.station
            .saturating_add(self.access_point)
            .saturating_add(self.foreign)
            .saturating_add(self.ambiguous)
            .saturating_add(self.malformed)
            .saturating_add(self.hardware_error)
    }

    fn record_rejected(&mut self, classification: Result<StaApRxRoute, RxError>) {
        match classification {
            Ok(StaApRxRoute::Foreign) => self.foreign = self.foreign.saturating_add(1),
            Ok(StaApRxRoute::Ambiguous) => self.ambiguous = self.ambiguous.saturating_add(1),
            Ok(StaApRxRoute::Malformed) => self.malformed = self.malformed.saturating_add(1),
            Ok(StaApRxRoute::Interface(_)) => {
                unreachable!("an addressed STA+AP frame cannot enter the rejected arm")
            }
            Err(_) => self.hardware_error = self.hardware_error.saturating_add(1),
        }
    }
}

/// Sole live DMA epoch plus the sole ordered STA/AP protocol consumer.
///
/// Role processors are deliberately not stored here. The outer paired DATAPATH
/// owns those processors together with their TX/control state and lends them
/// to one finite RX turn as disjoint mutable capabilities.
pub struct StaApRxService<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    dma: StagedRxEpoch<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
    protocol: StaApRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    serviced_frames: u64,
    routes: StaApRxRouteReport,
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    StaApRxService<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    pub const fn new(
        dma: StagedRxEpoch<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        protocol: StaApRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    ) -> Self {
        Self {
            dma,
            protocol,
            serviced_frames: 0,
            routes: StaApRxRouteReport::new(),
        }
    }

    pub const fn dma(
        &self,
    ) -> &StagedRxEpoch<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    > {
        &self.dma
    }

    pub async fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxStageTransactionError>
    where
        D: oer_time::Timer,
    {
        self.dma.start(hardware).await
    }

    pub fn stop<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxStageTransactionError>
    where
        D: oer_time::Timer,
    {
        self.dma.stop(hardware)
    }

    pub const fn serviced_frames(&self) -> u64 {
        self.serviced_frames
    }

    pub const fn route_report(&self) -> StaApRxRouteReport {
        self.routes
    }

    pub fn into_parts(
        self,
    ) -> (
        StagedRxEpoch<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        StaApRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    ) {
        (self.dma, self.protocol)
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M,
    H,
    PhysicalTx,
    Station,
    AccessPoint,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> DatapathPairedRxService<H, PhysicalTx, Station, AccessPoint>
    for StaApRxService<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
where
    M: RawMutex,
    D: oer_time::Timer,
    H: RxDma,
    Station: StaApStationRxRole<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
    AccessPoint: StaApAccessPointRxRole<'pool, H, PhysicalTx, STAGE_CAPACITY, STAGE_SLOTS>,
{
    type Error = StaApRxServiceError<Station::Error, AccessPoint::Error>;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        station: &'a mut Station,
        access_point: &'a mut AccessPoint,
        station_network: &'a mut dyn DatapathNetworkRx,
        access_point_network: &'a mut dyn DatapathNetworkRx,
        context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathPairedRxProgress, Self::Error>> + 'a {
        async move {
            let mut fused = FusedRxTurn::from_context(context, STAGE_SLOTS);
            loop {
                let now = self.dma.now().map_err(StaApRxServiceError::Dma)?;
                let network_backpressured = station.has_pending_rx()
                    && station
                        .publish_pending_rx(station_network)
                        .map_err(StaApRxServiceError::Station)?
                        == DatapathRxProgress::NetworkBackpressured
                    || access_point.has_pending_rx(now)
                        && access_point
                            .publish_pending_rx(physical_tx, access_point_network, now)
                            .map_err(StaApRxServiceError::AccessPoint)?
                            == DatapathRxProgress::NetworkBackpressured;
                if network_backpressured {
                    if fused.dma_service_required() {
                        let progress = self
                            .dma
                            .service(hardware)
                            .await
                            .map_err(StaApRxServiceError::Dma)?;
                        fused.observe_dma(progress);
                    }
                    return Ok(DatapathRxProgress::NetworkBackpressured.into());
                }

                if !fused.has_protocol_budget() {
                    if fused.dma_service_required() {
                        let progress = self
                            .dma
                            .service(hardware)
                            .await
                            .map_err(StaApRxServiceError::Dma)?;
                        fused.observe_dma(progress);
                    }
                    return Ok(fused.finish(self.protocol.queued_frames() != 0).into());
                }

                if self.protocol.queued_frames() == 0 {
                    if fused.dma_service_required() {
                        let progress = self
                            .dma
                            .service(hardware)
                            .await
                            .map_err(StaApRxServiceError::Dma)?;
                        fused.observe_dma(progress);
                        if self.protocol.queued_frames() != 0 {
                            continue;
                        }
                    }
                    return Ok(fused.finish(false).into());
                }

                let now = self.dma.now().map_err(StaApRxServiceError::Dma)?;
                let turn = self
                    .protocol
                    .service_next(
                        |frame| station.service_station_rx(frame, station_network),
                        |frame| {
                            access_point.service_access_point_rx(hardware, physical_tx, frame, now)
                        },
                    )
                    .await
                    .map_err(StaApRxServiceError::AccessPoint)?;
                match turn {
                    StaApRxTurn::Idle => continue,
                    StaApRxTurn::Station(dispatch) => {
                        dispatch.map_err(StaApRxServiceError::Station)?;
                        self.routes.station = self.routes.station.saturating_add(1);
                        self.serviced_frames = self.serviced_frames.saturating_add(1);
                        fused.observe_protocol(1, false);
                    }
                    StaApRxTurn::AccessPoint => {
                        self.routes.access_point = self.routes.access_point.saturating_add(1);
                        self.serviced_frames = self.serviced_frames.saturating_add(1);
                        fused.observe_protocol(1, false);
                    }
                    StaApRxTurn::DeferredAccessPoint => {
                        if fused.dma_service_required() {
                            let progress = self
                                .dma
                                .service(hardware)
                                .await
                                .map_err(StaApRxServiceError::Dma)?;
                            fused.observe_dma(progress);
                        }
                        return Ok(if access_point.tx_pending() {
                            DatapathPairedRxProgress::TxPending(DatapathPairRole::Second)
                        } else {
                            fused.finish(true).into()
                        });
                    }
                    StaApRxTurn::Rejected(classification) => {
                        self.routes.record_rejected(classification);
                        self.serviced_frames = self.serviced_frames.saturating_add(1);
                        fused.observe_protocol(1, false);
                    }
                }

                if access_point.tx_pending() {
                    if fused.dma_service_required() {
                        let progress = self
                            .dma
                            .service(hardware)
                            .await
                            .map_err(StaApRxServiceError::Dma)?;
                        fused.observe_dma(progress);
                    }
                    return Ok(DatapathPairedRxProgress::TxPending(
                        DatapathPairRole::Second,
                    ));
                }
            }
        }
    }

    fn service_during_tx<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        station: &'a mut Station,
        access_point: &'a mut AccessPoint,
        station_network: &'a mut dyn DatapathNetworkRx,
        access_point_network: &'a mut dyn DatapathNetworkRx,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            let mut fused = FusedRxTurn::new(ACTIVE_TX_PROTOCOL_QUANTUM_FRAMES);
            loop {
                let now = self.dma.now().map_err(StaApRxServiceError::Dma)?;
                let network_backpressured = station.has_pending_rx()
                    && station
                        .publish_pending_rx(station_network)
                        .map_err(StaApRxServiceError::Station)?
                        == DatapathRxProgress::NetworkBackpressured
                    || access_point.has_pending_rx(now)
                        && access_point
                            .publish_pending_rx(physical_tx, access_point_network, now)
                            .map_err(StaApRxServiceError::AccessPoint)?
                            == DatapathRxProgress::NetworkBackpressured;
                if network_backpressured {
                    if fused.dma_service_required() {
                        let progress = self
                            .dma
                            .service(hardware)
                            .await
                            .map_err(StaApRxServiceError::Dma)?;
                        fused.observe_dma(progress);
                    }
                    return Ok(DatapathRxProgress::NetworkBackpressured);
                }

                if !fused.has_protocol_budget() {
                    if fused.dma_service_required() {
                        let progress = self
                            .dma
                            .service(hardware)
                            .await
                            .map_err(StaApRxServiceError::Dma)?;
                        fused.observe_dma(progress);
                    }
                    return Ok(fused.finish(self.protocol.queued_frames() != 0));
                }

                if self.protocol.queued_frames() == 0 {
                    if fused.dma_service_required() {
                        let progress = self
                            .dma
                            .service(hardware)
                            .await
                            .map_err(StaApRxServiceError::Dma)?;
                        fused.observe_dma(progress);
                        if self.protocol.queued_frames() != 0 {
                            continue;
                        }
                    }
                    return Ok(fused.finish(false));
                }

                let now = self.dma.now().map_err(StaApRxServiceError::Dma)?;
                let turn = self
                    .protocol
                    .service_next(
                        |frame| station.service_station_rx(frame, station_network),
                        |frame| access_point.service_access_point_rx_during_tx(frame, now),
                    )
                    .await
                    .map_err(StaApRxServiceError::AccessPoint)?;
                match turn {
                    StaApRxTurn::Idle => continue,
                    StaApRxTurn::Station(dispatch) => {
                        dispatch.map_err(StaApRxServiceError::Station)?;
                        self.routes.station = self.routes.station.saturating_add(1);
                    }
                    StaApRxTurn::AccessPoint => {
                        self.routes.access_point = self.routes.access_point.saturating_add(1);
                    }
                    StaApRxTurn::DeferredAccessPoint => {
                        if fused.dma_service_required() {
                            let progress = self
                                .dma
                                .service(hardware)
                                .await
                                .map_err(StaApRxServiceError::Dma)?;
                            fused.observe_dma(progress);
                        }
                        return Ok(fused
                            .dma_progress()
                            .expect("paired active-TX turn services one DMA frontier"));
                    }
                    StaApRxTurn::Rejected(classification) => {
                        self.routes.record_rejected(classification);
                    }
                }
                self.serviced_frames = self.serviced_frames.saturating_add(1);
                fused.observe_protocol(1, false);
            }
        }
    }

    fn has_work(&self, station: &Station, access_point: &AccessPoint) -> bool {
        self.protocol.queued_frames() != 0
            || station.has_pending_rx()
            // A cancelled epoch transition has no clock; report work so the
            // next service turn surfaces the DMA error.
            || self
                .dma
                .now()
                .ok()
                .is_none_or(|now| access_point.has_pending_rx(now))
    }

    fn serviced_frames(&self) -> u64 {
        self.serviced_frames
    }

    fn work_counters(&self) -> DatapathRxWorkCounters {
        self.dma.work_counters()
    }
}
