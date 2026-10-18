#![expect(
    clippy::manual_async_fn,
    reason = "role implementations retain explicit borrowed Future contracts"
)]
#![expect(
    clippy::result_large_err,
    reason = "no-alloc station park and activation failures return the exact role owners"
)]

//! Connected-station protocol, TX and control ownership for paired DATAPATH.

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_ieee80211::{
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxResources},
    tx::WifiTxWake,
};

use oer_esp32s31_ieee80211_mac::tx::ampdu::HtAmpduHardware;

#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::aggregate_tx::PreparedTxSchedulerPhase;

use crate::{
    datapath::{
        DatapathControlContext, DatapathControlProgress, DatapathRxProgress, MaterializedTxFrame,
        PinnedTxFrame, SelectedBurstMaterializer, SoftwareTxFrame, TxBatchDemand, WifiTxProgress,
        network::DatapathNetworkRx,
        paired::{
            DatapathPairRole, DatapathPairedNetworkTxService, DatapathPairedPhysicalTx,
            DatapathPairedPhysicalTxError, DatapathPairedRoleOwner,
            DatapathPairedRoleTransitionError,
        },
        rx::staging::StagedRxFrame,
        services::{DatapathNetworkTxService, SingleRoleServices},
        tx::resources::AggregateTxResources,
    },
    roles::{
        concurrent::{StaApStationControlRole, StaApStationRxRole},
        station::{
            connected::port::{ConnectedStaDrivers, ConnectedStaReport},
            control::{ConnectedControlError, ConnectedControlHardware},
            tx::{AggregateTxError, ConnectedTx, ConnectedTxParked},
        },
    },
};

/// Connected STA owners after the standalone runner boundary has been
/// removed and the physical TX pair has been parked in the shared owner.
///
/// The physical RX transport remains separate because the paired runtime
/// replaces it with one common STA/AP classifier. The station role retains
/// only its protocol consumer, connected control, and parked TX state.
pub struct StaApStationPrepared<H, PhysicalRx, Station, PhysicalTx> {
    pub hardware: H,
    pub physical_rx: PhysicalRx,
    pub station: Station,
    pub physical_tx: PhysicalTx,
    pub report: ConnectedStaReport,
}

/// Fail-closed station preparation frontier.
///
/// A busy connected TX transaction prevents paired composition. Every owner
/// is returned in its original active station form; callers cannot continue
/// with a partly materialized shared TX graph.
pub struct StaApStationPrepareFailure<H, PhysicalRx, Station> {
    pub hardware: H,
    pub physical_rx: PhysicalRx,
    pub station: Station,
    pub report: ConnectedStaReport,
}

/// Why a paired station cannot return to the ordinary connected owner graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApStationFinishReason {
    StationTxActive,
    PhysicalTxLent(DatapathPairRole),
}

/// Fail-closed paired-to-connected frontier.
///
/// The complete prepared graph is retained unchanged.  A caller may finish
/// the outstanding transaction and retry; it never has to manufacture a TX
/// owner from policy state.
pub struct StaApStationFinishFailure<H, PhysicalRx, Station, PhysicalTx> {
    pub reason: StaApStationFinishReason,
    pub prepared: StaApStationPrepared<H, PhysicalRx, Station, PhysicalTx>,
}

/// One station role owner lent independently to common RX, TX and control
/// turns. None of its fields owns physical DMA, IRQ or network endpoints.
pub type StationRoleRuntime<Rx, Tx, Control> =
    crate::datapath::services::RoleRuntime<Rx, Tx, Control>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApStationTxOwnershipError {
    AlreadyActive,
    AlreadyParked,
    Busy,
    Physical(DatapathPairedPhysicalTxError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApStationTxError {
    Operation(AggregateTxError),
    Ownership(StaApStationTxOwnershipError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApStationControlError {
    Operation(ConnectedControlError),
    Ownership(StaApStationTxOwnershipError),
}

type StaApStationBacking<
    'resources,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;

type StaApConnectedTx<
    'resources,
    'slot,
    'ampdu,
    M,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> = ConnectedTx<
    'slot,
    'ampdu,
    crate::datapath::PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    P,
    E,
    T,
    SLOTS,
    AMPDU_BUFFER_SIZE,
    ORDINARY_BUFFER_SIZE,
>;

type StaApStationPhysicalTx<
    'resources,
    'slot,
    'ampdu,
    M,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> = DatapathPairedPhysicalTx<
    WifiTxResources<'slot, P, E, T, ORDINARY_BUFFER_SIZE>,
    AggregateTxResources<
        'ampdu,
        StaApStationBacking<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        SLOTS,
        AMPDU_BUFFER_SIZE,
    >,
>;

/// Establish the station side of the paired ownership graph. The returned
/// role retains association policy only; the exact ordinary/A-MPDU pair is
/// held once by `DatapathPairedPhysicalTx`.
#[allow(clippy::type_complexity)]
pub fn park_sta_ap_station_role<
    'resources,
    'slot,
    'ampdu,
    M,
    P,
    E,
    T,
    Rx,
    Control,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
>(
    role: StationRoleRuntime<
        Rx,
        ConnectedTx<
            'slot,
            'ampdu,
            crate::datapath::PinnedTxFrame<
                'resources,
                M,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
            >,
            P,
            E,
            T,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        Control,
    >,
) -> Result<
    (
        StationRoleRuntime<
            Rx,
            DatapathPairedRoleOwner<
                ConnectedTx<
                    'slot,
                    'ampdu,
                    crate::datapath::PinnedTxFrame<
                        'resources,
                        M,
                        FRAME_CAPACITY,
                        HEADROOM,
                        TRAILER,
                        QUEUE_DEPTH,
                    >,
                    P,
                    E,
                    T,
                    SLOTS,
                    AMPDU_BUFFER_SIZE,
                    ORDINARY_BUFFER_SIZE,
                >,
                ConnectedTxParked<'ampdu, SLOTS>,
            >,
            Control,
        >,
        StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    ),
    StationRoleRuntime<
        Rx,
        ConnectedTx<
            'slot,
            'ampdu,
            crate::datapath::PinnedTxFrame<
                'resources,
                M,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
            >,
            P,
            E,
            T,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        Control,
    >,
>
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    'resources: 'ampdu,
{
    let (rx, tx, control) = role.into_parts();
    match tx.try_park() {
        Ok((ordinary, aggregate, parked)) => Ok((
            StationRoleRuntime::new(rx, DatapathPairedRoleOwner::parked(parked), control),
            DatapathPairedPhysicalTx::new(ordinary, aggregate),
        )),
        Err(tx) => Err(StationRoleRuntime::new(rx, tx, control)),
    }
}

/// Remove the standalone connected-services boundary and establish the STA
/// half of one paired DATAPATH composition.
///
/// This is the cutover transaction used by production integration: hardware
/// and DMA stay role-neutral, station protocol/control become one logical
/// role, and ordinary plus A-MPDU resources acquire exactly one physical
/// owner. No network or interrupt runner is created here.
#[allow(clippy::type_complexity)]
pub fn prepare_sta_ap_station<
    'resources,
    'slot,
    'ampdu,
    M,
    P,
    E,
    T,
    H,
    PhysicalRx,
    Protocol,
    Control,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
>(
    drivers: ConnectedStaDrivers<
        H,
        PhysicalRx,
        ConnectedTx<
            'slot,
            'ampdu,
            crate::datapath::PinnedTxFrame<
                'resources,
                M,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
            >,
            P,
            E,
            T,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        Control,
        Protocol,
    >,
) -> Result<
    StaApStationPrepared<
        H,
        PhysicalRx,
        StationRoleRuntime<
            Protocol,
            DatapathPairedRoleOwner<
                ConnectedTx<
                    'slot,
                    'ampdu,
                    crate::datapath::PinnedTxFrame<
                        'resources,
                        M,
                        FRAME_CAPACITY,
                        HEADROOM,
                        TRAILER,
                        QUEUE_DEPTH,
                    >,
                    P,
                    E,
                    T,
                    SLOTS,
                    AMPDU_BUFFER_SIZE,
                    ORDINARY_BUFFER_SIZE,
                >,
                ConnectedTxParked<'ampdu, SLOTS>,
            >,
            Control,
        >,
        StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    >,
    StaApStationPrepareFailure<
        H,
        PhysicalRx,
        StationRoleRuntime<
            Protocol,
            ConnectedTx<
                'slot,
                'ampdu,
                crate::datapath::PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                P,
                E,
                T,
                SLOTS,
                AMPDU_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
            Control,
        >,
    >,
>
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    'resources: 'ampdu,
{
    let ConnectedStaDrivers { services, report } = drivers;
    let (hardware, rx, tx, control) = services.into_parts();
    let (physical_rx, protocol) = rx.into_parts();
    match park_sta_ap_station_role(StationRoleRuntime::new(protocol, tx, control)) {
        Ok((station, physical_tx)) => Ok(StaApStationPrepared {
            hardware,
            physical_rx,
            station,
            physical_tx,
            report,
        }),
        Err(station) => Err(StaApStationPrepareFailure {
            hardware,
            physical_rx,
            station,
            report,
        }),
    }
}

/// Rejoin a quiescent paired STA role with the exact physical TX resources
/// removed by [`prepare_sta_ap_station`].
///
/// This is the sole inverse cutover transaction.  It succeeds only after the
/// paired DATAPATH has reached an idle terminal edge: neither logical role may
/// retain ordinary or aggregate hardware authority.
#[allow(clippy::type_complexity)]
pub fn finish_sta_ap_station<
    'resources,
    'slot,
    'ampdu,
    M,
    P,
    E,
    T,
    H,
    PhysicalRx,
    Protocol,
    Control,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
>(
    prepared: StaApStationPrepared<
        H,
        PhysicalRx,
        StationRoleRuntime<
            Protocol,
            DatapathPairedRoleOwner<
                ConnectedTx<
                    'slot,
                    'ampdu,
                    crate::datapath::PinnedTxFrame<
                        'resources,
                        M,
                        FRAME_CAPACITY,
                        HEADROOM,
                        TRAILER,
                        QUEUE_DEPTH,
                    >,
                    P,
                    E,
                    T,
                    SLOTS,
                    AMPDU_BUFFER_SIZE,
                    ORDINARY_BUFFER_SIZE,
                >,
                ConnectedTxParked<'ampdu, SLOTS>,
            >,
            Control,
        >,
        StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    >,
) -> Result<
    ConnectedStaDrivers<
        H,
        PhysicalRx,
        ConnectedTx<
            'slot,
            'ampdu,
            crate::datapath::PinnedTxFrame<
                'resources,
                M,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
            >,
            P,
            E,
            T,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        Control,
        Protocol,
    >,
    StaApStationFinishFailure<
        H,
        PhysicalRx,
        StationRoleRuntime<
            Protocol,
            DatapathPairedRoleOwner<
                ConnectedTx<
                    'slot,
                    'ampdu,
                    crate::datapath::PinnedTxFrame<
                        'resources,
                        M,
                        FRAME_CAPACITY,
                        HEADROOM,
                        TRAILER,
                        QUEUE_DEPTH,
                    >,
                    P,
                    E,
                    T,
                    SLOTS,
                    AMPDU_BUFFER_SIZE,
                    ORDINARY_BUFFER_SIZE,
                >,
                ConnectedTxParked<'ampdu, SLOTS>,
            >,
            Control,
        >,
        StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    >,
>
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    'resources: 'ampdu,
{
    if !prepared.station.tx().is_parked() {
        return Err(StaApStationFinishFailure {
            reason: StaApStationFinishReason::StationTxActive,
            prepared,
        });
    }
    if let Some(role) = prepared.physical_tx.lent_to() {
        return Err(StaApStationFinishFailure {
            reason: StaApStationFinishReason::PhysicalTxLent(role),
            prepared,
        });
    }

    let StaApStationPrepared {
        hardware,
        physical_rx,
        station,
        physical_tx,
        report,
    } = prepared;
    let (protocol, tx, control) = station.into_parts();
    let parked = tx
        .try_into_parked()
        .unwrap_or_else(|_| unreachable!("station TX was checked quiescent"));
    let (ordinary, aggregate) = physical_tx
        .try_into_resources()
        .unwrap_or_else(|_| unreachable!("physical TX was checked available"));
    let tx = ConnectedTx::resume(ordinary, aggregate, parked);

    Ok(ConnectedStaDrivers {
        services: SingleRoleServices::with_control(
            hardware,
            crate::roles::station::connected::ConnectedStaRxService::new(physical_rx, protocol),
            tx,
            control,
        ),
        report,
    })
}

impl<
    'resources,
    'slot,
    'ampdu,
    M,
    P,
    E,
    T,
    Rx,
    Control,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
>
    StationRoleRuntime<
        Rx,
        DatapathPairedRoleOwner<
            ConnectedTx<
                'slot,
                'ampdu,
                crate::datapath::PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                P,
                E,
                T,
                SLOTS,
                AMPDU_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
            ConnectedTxParked<'ampdu, SLOTS>,
        >,
        Control,
    >
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    'resources: 'ampdu,
{
    fn activate_tx(
        &mut self,
        physical: &mut StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    ) -> Result<(), StaApStationTxOwnershipError> {
        if !self.tx_mut().is_parked() {
            return Err(StaApStationTxOwnershipError::AlreadyActive);
        }
        let (ordinary, aggregate) = physical
            .try_lend(DatapathPairRole::First)
            .map_err(StaApStationTxOwnershipError::Physical)?;
        self.tx_mut()
            .try_activate(|parked| {
                Ok::<_, (core::convert::Infallible, _)>(ConnectedTx::resume(
                    ordinary, aggregate, parked,
                ))
            })
            .map_err(|error| match error {
                DatapathPairedRoleTransitionError::AlreadyActive => {
                    StaApStationTxOwnershipError::AlreadyActive
                }
                DatapathPairedRoleTransitionError::AlreadyParked => {
                    StaApStationTxOwnershipError::AlreadyParked
                }
                DatapathPairedRoleTransitionError::Conversion(never) => match never {},
            })
    }

    fn park_tx(
        &mut self,
        physical: &mut StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    ) -> Result<(), StaApStationTxOwnershipError> {
        if self.tx_mut().is_parked() {
            return Err(StaApStationTxOwnershipError::AlreadyParked);
        }
        self.tx_mut()
            .try_park(|active| match active.try_park() {
                Ok((ordinary, aggregate, parked)) => {
                    match physical.restore(DatapathPairRole::First, ordinary, aggregate) {
                        Ok(()) => Ok(parked),
                        Err((error, ordinary, aggregate)) => Err((
                            StaApStationTxOwnershipError::Physical(error),
                            ConnectedTx::resume(ordinary, aggregate, parked),
                        )),
                    }
                }
                Err(active) => Err((StaApStationTxOwnershipError::Busy, active)),
            })
            .map_err(|error| match error {
                DatapathPairedRoleTransitionError::AlreadyActive => {
                    StaApStationTxOwnershipError::AlreadyActive
                }
                DatapathPairedRoleTransitionError::AlreadyParked => {
                    StaApStationTxOwnershipError::AlreadyParked
                }
                DatapathPairedRoleTransitionError::Conversion(error) => error,
            })
    }
}

impl<
    'resources,
    'slot,
    'ampdu,
    'control,
    M,
    H,
    P,
    E,
    T,
    Rx,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
    const CONTROL_CAPACITY: usize,
>
    StaApStationControlRole<
        H,
        StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    >
    for StationRoleRuntime<
        Rx,
        DatapathPairedRoleOwner<
            ConnectedTx<
                'slot,
                'ampdu,
                crate::datapath::PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                P,
                E,
                T,
                SLOTS,
                AMPDU_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
            ConnectedTxParked<'ampdu, SLOTS>,
        >,
        crate::roles::station::control_slot::PlacedConnectedControl<'control, M, CONTROL_CAPACITY>,
    >
where
    M: RawMutex,
    H: ConnectedControlHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    'resources: 'ampdu,
{
    type Error = StaApStationControlError;
    type Exit = oer_esp32s31_ieee80211_sta::connected_control::ConnectedDisconnectReason;

    fn service_station_control<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical: &'a mut StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        context: DatapathControlContext,
        retain_physical_tx: bool,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a {
        async move {
            if self.tx_mut().is_parked() {
                self.activate_tx(physical)
                    .map_err(StaApStationControlError::Ownership)?;
            }
            let result = {
                let (control, tx) = self.control_and_tx_mut();
                control
                    .service_with_context(
                        hardware,
                        tx.active_mut()
                            .expect("station control owns the physical TX pair"),
                        context,
                    )
                    .await
                    .map_err(StaApStationControlError::Operation)
            };
            let tx_pending = self.tx_mut().active().is_some_and(ConnectedTx::active);
            if !tx_pending
                && !retain_physical_tx
                && let Err(error) = self.park_tx(physical)
                && result.is_ok()
            {
                return Err(StaApStationControlError::Ownership(error));
            }
            result
        }
    }

    fn station_control_ready(&self, now: oer_time::Instant) -> bool {
        self.control().has_immediate_work()
            || self
                .control()
                .next_alarm_deadline()
                .is_some_and(|deadline| deadline <= now)
    }

    fn station_required_before_stop(&self) -> bool {
        self.control().leave_pending()
    }

    fn wait_station_control_ready<'a>(
        &'a mut self,
        timer: &'a impl oer_time::Timer,
    ) -> impl Future<Output = ()> + 'a {
        self.control_mut().wait_ready_without_tx(timer)
    }
}

impl<'pool, Rx, Tx, Control, const CAPACITY: usize, const SLOTS: usize>
    StaApStationRxRole<'pool, CAPACITY, SLOTS> for StationRoleRuntime<Rx, Tx, Control>
where
    Rx: StaApStationRxRole<'pool, CAPACITY, SLOTS>,
{
    type Dispatch = Rx::Dispatch;
    type Error = Rx::Error;

    fn publish_pending_rx(
        &mut self,
        network: &mut dyn DatapathNetworkRx,
    ) -> Result<DatapathRxProgress, Self::Error> {
        self.rx_mut().publish_pending_rx(network)
    }

    fn service_station_rx<'a>(
        &'a mut self,
        frame: StagedRxFrame<'pool, CAPACITY, SLOTS>,
        network: &'a mut dyn DatapathNetworkRx,
    ) -> impl Future<Output = Result<Self::Dispatch, Self::Error>> + 'a
    where
        'pool: 'a,
    {
        self.rx_mut().service_station_rx(frame, network)
    }

    fn has_pending_rx(&self) -> bool {
        self.rx().has_pending_rx()
    }
}

impl<
    'resources,
    'slot,
    'ampdu,
    M,
    H,
    P,
    E,
    T,
    Rx,
    Control,
    SoftwareFrame,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
>
    DatapathPairedNetworkTxService<
        H,
        StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        SoftwareFrame,
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    >
    for StationRoleRuntime<
        Rx,
        DatapathPairedRoleOwner<
            ConnectedTx<
                'slot,
                'ampdu,
                crate::datapath::PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                P,
                E,
                T,
                SLOTS,
                AMPDU_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            >,
            ConnectedTxParked<'ampdu, SLOTS>,
        >,
        Control,
    >
where
    M: RawMutex,
    H: HtAmpduHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + crate::mac_clock::MacClockReader,
    'resources: 'ampdu,
    SoftwareFrame: SoftwareTxFrame,
{
    type Error = StaApStationTxError;

    fn last_started_frame_count(&self) -> usize {
        self.tx()
            .active()
            .map_or(1, ConnectedTx::active_network_frame_count)
    }

    fn start<'a, I>(
        &'a mut self,
        hardware: &'a mut H,
        physical: &'a mut StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        frame: SoftwareFrame,
        network: &'a I,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a
    where
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<
                SoftwareFrame = SoftwareFrame,
                PhysicalFrame = PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
            > + 'a,
    {
        async move {
            if self.tx_mut().is_parked() {
                self.activate_tx(physical)
                    .map_err(StaApStationTxError::Ownership)?;
            }
            let progress = self
                .tx_mut()
                .active_mut()
                .expect("station network TX owns the physical pair")
                .start(hardware, frame, network)
                .await
                .map_err(StaApStationTxError::Operation)?;
            let retained = self
                .tx()
                .active()
                .is_some_and(ConnectedTx::has_prepared_network_tx);
            if progress == WifiTxProgress::Complete && !retained {
                self.park_tx(physical)
                    .map_err(StaApStationTxError::Ownership)?;
            }
            Ok(progress)
        }
    }

    fn wait_deadline<'a>(
        &'a mut self,
        _physical: &'a mut StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    ) -> impl Future<Output = ()> + 'a {
        async move {
            self.tx_mut()
                .active_mut()
                .expect("paired scheduler retains station TX until terminal")
                .wait_deadline()
                .await;
        }
    }

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical: &'a mut StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let progress = self
                .tx_mut()
                .active_mut()
                .ok_or(StaApStationTxError::Ownership(
                    StaApStationTxOwnershipError::AlreadyParked,
                ))?
                .service(hardware, wake)
                .map_err(StaApStationTxError::Operation)?;
            let retained = self
                .tx()
                .active()
                .is_some_and(ConnectedTx::has_prepared_network_tx);
            if progress == WifiTxProgress::Complete && !retained {
                self.park_tx(physical)
                    .map_err(StaApStationTxError::Ownership)?;
            }
            Ok(progress)
        }
    }

    fn has_prepared(&self) -> bool {
        self.tx()
            .active()
            .is_some_and(ConnectedTx::has_prepared_network_tx)
    }

    fn batch_demand<I>(&self, network: &I) -> TxBatchDemand
    where
        I: SelectedBurstMaterializer<
                SoftwareFrame = SoftwareFrame,
                PhysicalFrame = PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
            >,
    {
        self.tx().active().map_or_else(
            || TxBatchDemand::single(network.queue_len()),
            |tx| tx.network_batch_demand(network.queue_len()),
        )
    }

    fn prepared_frame_count(&self) -> usize {
        self.tx()
            .active()
            .map_or(0, ConnectedTx::prepared_network_frame_count)
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn mark_prepared_scheduler_phase(&mut self, phase: PreparedTxSchedulerPhase, at_micros: u64) {
        if let Some(tx) = self.tx_mut().active_mut() {
            tx.mark_prepared_scheduler_phase(phase, at_micros);
        }
    }

    fn start_prepared<I>(
        &mut self,
        hardware: &mut H,
        physical: &mut StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        network: &I,
    ) -> Result<WifiTxProgress, Self::Error>
    where
        I: SelectedBurstMaterializer<
                SoftwareFrame = SoftwareFrame,
                PhysicalFrame = PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
            >,
    {
        let progress = self
            .tx_mut()
            .active_mut()
            .ok_or(StaApStationTxError::Ownership(
                StaApStationTxOwnershipError::AlreadyParked,
            ))?
            .start_prepared(hardware, network)
            .map_err(StaApStationTxError::Operation)?;
        let retained = self
            .tx()
            .active()
            .is_some_and(ConnectedTx::has_prepared_network_tx);
        if progress == WifiTxProgress::Complete && !retained {
            self.park_tx(physical)
                .map_err(StaApStationTxError::Ownership)?;
        }
        Ok(progress)
    }

    fn cancel_prepared<I>(
        &mut self,
        physical: &mut StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        network: &I,
    ) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<
                SoftwareFrame = SoftwareFrame,
                PhysicalFrame = PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
            >,
    {
        <StaApConnectedTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        > as DatapathNetworkTxService<
            H,
            SoftwareFrame,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        >>::cancel_prepared(
            self.tx_mut()
                .active_mut()
                .ok_or(StaApStationTxError::Ownership(
                    StaApStationTxOwnershipError::AlreadyParked,
                ))?,
            network,
        )
        .map_err(StaApStationTxError::Operation)?;
        self.park_tx(physical)
            .map_err(StaApStationTxError::Ownership)
    }

    fn can_prepare(
        &self,
        _physical: &StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
    ) -> bool {
        self.tx().active().is_some_and(|active| {
            <StaApConnectedTx<
                'resources,
                'slot,
                'ampdu,
                M,
                P,
                E,
                T,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
                SLOTS,
                AMPDU_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            > as DatapathNetworkTxService<
                H,
                SoftwareFrame,
                PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
            >>::can_prepare(active)
        })
    }

    fn prepare<'a, I>(
        &'a mut self,
        physical: &'a mut StaApStationPhysicalTx<
            'resources,
            'slot,
            'ampdu,
            M,
            P,
            E,
            T,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
            SLOTS,
            AMPDU_BUFFER_SIZE,
            ORDINARY_BUFFER_SIZE,
        >,
        frame: SoftwareFrame,
        network: &'a I,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        H: 'a,
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<
                SoftwareFrame = SoftwareFrame,
                PhysicalFrame = PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
            > + 'a,
    {
        async move {
            if self.tx_mut().is_parked() {
                self.activate_tx(physical)
                    .map_err(StaApStationTxError::Ownership)?;
            }
            <StaApConnectedTx<
                'resources,
                'slot,
                'ampdu,
                M,
                P,
                E,
                T,
                FRAME_CAPACITY,
                HEADROOM,
                TRAILER,
                QUEUE_DEPTH,
                SLOTS,
                AMPDU_BUFFER_SIZE,
                ORDINARY_BUFFER_SIZE,
            > as DatapathNetworkTxService<
                H,
                SoftwareFrame,
                PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
            >>::prepare(
                self.tx_mut()
                    .active_mut()
                    .expect("station prepare owns the physical pair"),
                frame,
                network,
            )
            .await
            .map_err(StaApStationTxError::Operation)
        }
    }
}

impl<H, Rx, Tx, Control, SoftwareFrame, PhysicalFrame>
    DatapathNetworkTxService<H, SoftwareFrame, PhysicalFrame>
    for StationRoleRuntime<Rx, Tx, Control>
where
    SoftwareFrame: SoftwareTxFrame,
    PhysicalFrame: MaterializedTxFrame,
    Tx: DatapathNetworkTxService<H, SoftwareFrame, PhysicalFrame>,
{
    type Error = Tx::Error;

    fn start<'a, I>(
        &'a mut self,
        hardware: &'a mut H,
        frame: SoftwareFrame,
        network: &'a I,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a
    where
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>
            + 'a,
    {
        self.tx_mut().start(hardware, frame, network)
    }

    fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        self.tx_mut().wait_deadline()
    }

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        self.tx_mut().service(hardware, wake)
    }

    fn has_prepared(&self) -> bool {
        self.tx().has_prepared()
    }

    fn batch_demand<I>(&self, network: &I) -> TxBatchDemand
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        self.tx().batch_demand(network)
    }

    fn prepared_frame_count(&self) -> usize {
        self.tx().prepared_frame_count()
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn mark_prepared_scheduler_phase(&mut self, phase: PreparedTxSchedulerPhase, at_micros: u64) {
        self.tx_mut()
            .mark_prepared_scheduler_phase(phase, at_micros);
    }

    fn start_prepared<I>(
        &mut self,
        hardware: &mut H,
        network: &I,
    ) -> Result<WifiTxProgress, Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        self.tx_mut().start_prepared(hardware, network)
    }

    fn cancel_prepared<I>(&mut self, network: &I) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        self.tx_mut().cancel_prepared(network)
    }

    fn can_prepare(&self) -> bool {
        self.tx().can_prepare()
    }

    fn prepare<'a, I>(
        &'a mut self,
        frame: SoftwareFrame,
        network: &'a I,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        H: 'a,
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>
            + 'a,
    {
        self.tx_mut().prepare(frame, network)
    }
}
