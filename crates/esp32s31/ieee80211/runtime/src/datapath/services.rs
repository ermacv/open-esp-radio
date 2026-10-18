#![expect(
    clippy::manual_async_fn,
    reason = "service implementations keep explicit borrowed Future contracts across role adapters"
)]

//! Finite role-neutral RX, control and network-TX service composition.

use core::{
    convert::Infallible,
    future::{Future, pending, ready},
};

#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::aggregate_tx::PreparedTxSchedulerPhase;
use oer_network_interface::NetworkInterfaceId;

use crate::datapath::{
    DatapathControlContext, DatapathControlProgress, DatapathRxProgress, DatapathRxServiceContext,
    DatapathRxWorkCounters, DatapathServices, SelectedBurstMaterializer, SoftwareTxFrame,
    TxBatchDemand, WifiTxProgress, WifiTxWake, network::DatapathNetworkRxSet,
};

/// One bounded RX owner spanning physical descriptor service and any
/// role-local protocol consumer.
///
/// A DMA-only role may ignore `network_rx` and `context`. Connected roles keep
/// their protocol processor in this same owner so one executor turn can stage,
/// recycle and dispatch a finite batch without an intermediate task wake.
/// Implementations may await bounded reload or protocol timer edges but must
/// not retain the mutable hardware borrow across an unrelated executor turn.
pub trait DatapathRxService<H> {
    type Error: 'static;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a;

    /// Service one complete physical/protocol turn. DMA-only owners retain the
    /// narrow historical primitive above; a connected owner overrides this
    /// method to consume the staged frames in the same executor turn.
    fn service_turn<'a>(
        &'a mut self,
        hardware: &'a mut H,
        network_rx: &'a mut dyn DatapathNetworkRxSet,
        context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        let _ = (network_rx, context);
        self.service(hardware)
    }

    /// Bounded RX work permitted while TX owns the physical transaction.
    fn service_turn_during_tx<'a>(
        &'a mut self,
        hardware: &'a mut H,
        network_rx: &'a mut dyn DatapathNetworkRxSet,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        self.service_turn(
            hardware,
            network_rx,
            DatapathRxServiceContext {
                maximum_protocol_frames: None,
            },
        )
    }

    /// Software-owned frames, commands or expired deadlines ready without a
    /// fresh MAC interrupt.
    fn has_work(&self) -> bool {
        false
    }

    /// Monotonic number of staged protocol frames consumed by this owner.
    fn serviced_frames(&self) -> u64 {
        0
    }

    /// Monotonic physical units and bytes completed by this RX owner.
    fn work_counters(&self) -> DatapathRxWorkCounters {
        DatapathRxWorkCounters::default()
    }

    /// Wake edge for a future protocol deadline. Queue publications paired
    /// with a MAC interrupt need no second wake source.
    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        pending()
    }
}

/// One owned control scheduler sharing the services owner's PAC and TX transaction.
pub trait DatapathControlService<H, X> {
    type Error: 'static;
    type Exit: 'static;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a;

    /// O(1) readiness observation used at a physical TX boundary.
    fn ready(&self, _tx: &X, _now: oer_time::Instant) -> bool {
        true
    }

    /// Require one control transition before the scheduler may claim a new
    /// network TX lease (for example, to advertise PM=0 first).
    fn required_before_network_tx(&self) -> bool {
        false
    }

    fn required_before_stop(&self) -> bool {
        false
    }

    /// Whether the scheduler may publish a queued network frame now. A
    /// station in power save holds its frames until it leaves it.
    fn admits_network_tx(&self) -> bool {
        true
    }

    fn wait_ready<'a>(&'a mut self, tx: &'a mut X) -> impl Future<Output = ()> + 'a;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoDatapathControl;

impl<H, X> DatapathControlService<H, X> for NoDatapathControl {
    type Error = Infallible;
    type Exit = Infallible;

    fn service<'a>(
        &'a mut self,
        _hardware: &'a mut H,
        _tx: &'a mut X,
        _context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a {
        ready(Ok(DatapathControlProgress::Idle))
    }

    fn ready(&self, _tx: &X, _now: oer_time::Instant) -> bool {
        false
    }

    fn wait_ready<'a>(&'a mut self, _tx: &'a mut X) -> impl Future<Output = ()> + 'a {
        pending()
    }
}

/// Connected network-TX transaction owned by the connected services graph.
///
/// Taking the first lease by value is essential for referenced DMA. The
/// service may synchronously claim further ready leases from `network`, but
/// never retains the runner capability itself.
pub trait DatapathNetworkTxService<H, SoftwareFrame, PhysicalFrame>
where
    SoftwareFrame: SoftwareTxFrame,
    PhysicalFrame: crate::datapath::MaterializedTxFrame,
{
    type Error: 'static;

    fn start<'a, I>(
        &'a mut self,
        hardware: &'a mut H,
        frame: SoftwareFrame,
        network: &'a I,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a
    where
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>
            + 'a;

    /// Network leases claimed by the most recent successful start.
    fn last_started_frame_count(&self) -> usize {
        1
    }

    fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a;

    /// Whether a software-owned network batch is ready for scheduler
    /// admission. It is not hardware-owned and must not suppress control
    /// service at the transaction boundary.
    fn has_prepared(&self) -> bool {
        false
    }

    /// Aggregation demand of this role's next network batch.
    fn batch_demand<I>(&self, network: &I) -> TxBatchDemand
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        TxBatchDemand::single(self.prepared_frame_count() + network.queue_len())
    }

    fn prepared_frame_count(&self) -> usize {
        0
    }

    fn prepared_start_ready(&self) -> bool {
        self.has_prepared()
    }

    fn advance_prepared<I>(&mut self, _hardware: &mut H, _network: &I) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        Ok(())
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn mark_prepared_scheduler_phase(&mut self, _phase: PreparedTxSchedulerPhase, _at_micros: u64) {
    }

    fn start_prepared<I>(
        &mut self,
        _hardware: &mut H,
        _network: &I,
    ) -> Result<WifiTxProgress, Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        Ok(WifiTxProgress::Complete)
    }

    /// Release a software-owned standby batch at stop/disconnect.
    fn cancel_prepared<I>(&mut self, _network: &I) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        Ok(())
    }

    fn can_prepare(&self) -> bool {
        false
    }

    fn prepare<'a, I>(
        &'a mut self,
        _frame: SoftwareFrame,
        _network: &'a I,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        H: 'a,
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>
            + 'a,
    {
        ready(Ok(()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathServiceError<RxError, ControlError = Infallible, TxError = Infallible> {
    Rx(RxError),
    Control(ControlError),
    Tx(TxError),
}

/// Role-local RX, TX and control ownership independent of the physical
/// register capability.
///
/// A standalone composition stores this value beside its hardware owner. A
/// concurrent composition moves the same value into one logical interface
/// slot. The container intentionally has no STA/AP policy of its own.
pub struct RoleRuntime<R, X, C> {
    rx: R,
    tx: X,
    control: C,
}

impl<R, X, C> RoleRuntime<R, X, C> {
    pub const fn new(rx: R, tx: X, control: C) -> Self {
        Self { rx, tx, control }
    }

    pub const fn rx(&self) -> &R {
        &self.rx
    }

    pub fn rx_mut(&mut self) -> &mut R {
        &mut self.rx
    }

    pub const fn tx(&self) -> &X {
        &self.tx
    }

    pub fn tx_mut(&mut self) -> &mut X {
        &mut self.tx
    }

    pub const fn control(&self) -> &C {
        &self.control
    }

    pub fn control_mut(&mut self) -> &mut C {
        &mut self.control
    }

    pub fn control_and_tx_mut(&mut self) -> (&mut C, &mut X) {
        (&mut self.control, &mut self.tx)
    }

    pub fn into_parts(self) -> (R, X, C) {
        (self.rx, self.tx, self.control)
    }
}

/// Unique connected-epoch owner for the shared register capability, RX ring,
/// connected control and the selected ordinary/aggregate TX owner.
pub struct SingleRoleServices<H, R, X, C = NoDatapathControl> {
    hardware: H,
    role: RoleRuntime<R, X, C>,
}

impl<H, R, X> SingleRoleServices<H, R, X, NoDatapathControl> {
    pub const fn new(hardware: H, rx: R, tx: X) -> Self {
        Self {
            hardware,
            role: RoleRuntime::new(rx, tx, NoDatapathControl),
        }
    }
}

impl<H, R, X, C> SingleRoleServices<H, R, X, C> {
    /// Split unique borrows for a finite control exchange at a paused boundary.
    pub fn control_parts_mut(&mut self) -> (&mut H, &mut X, &mut C) {
        (
            &mut self.hardware,
            &mut self.role.tx,
            &mut self.role.control,
        )
    }

    /// Preserve hardware, TX and control even if the RX transition fails.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_map_rx<T, E>(
        self,
        map: impl FnOnce(&mut H, R) -> Result<T, E>,
    ) -> Result<SingleRoleServices<H, T, X, C>, SingleRoleServices<H, E, X, C>> {
        let (mut hardware, rx, tx, control) = self.into_parts();
        match map(&mut hardware, rx) {
            Ok(rx) => Ok(SingleRoleServices::with_control(hardware, rx, tx, control)),
            Err(rx) => Err(SingleRoleServices::with_control(hardware, rx, tx, control)),
        }
    }

    /// Change only RX ownership while retaining hardware, TX and control.
    /// The caller establishes TX-idle/MAC/IRQ boundaries before a DMA pause.
    pub fn map_rx<T>(self, map: impl FnOnce(&mut H, R) -> T) -> SingleRoleServices<H, T, X, C> {
        let (mut hardware, rx, tx, control) = self.into_parts();
        let rx = map(&mut hardware, rx);
        SingleRoleServices::with_control(hardware, rx, tx, control)
    }

    pub const fn with_control(hardware: H, rx: R, tx: X, control: C) -> Self {
        Self {
            hardware,
            role: RoleRuntime::new(rx, tx, control),
        }
    }

    pub const fn hardware(&self) -> &H {
        &self.hardware
    }

    pub fn hardware_mut(&mut self) -> &mut H {
        &mut self.hardware
    }

    pub const fn rx(&self) -> &R {
        self.role.rx()
    }

    pub fn rx_mut(&mut self) -> &mut R {
        self.role.rx_mut()
    }

    pub const fn tx(&self) -> &X {
        self.role.tx()
    }

    pub fn tx_mut(&mut self) -> &mut X {
        self.role.tx_mut()
    }

    pub const fn control(&self) -> &C {
        self.role.control()
    }

    pub fn control_mut(&mut self) -> &mut C {
        self.role.control_mut()
    }

    pub const fn role(&self) -> &RoleRuntime<R, X, C> {
        &self.role
    }

    pub fn role_mut(&mut self) -> &mut RoleRuntime<R, X, C> {
        &mut self.role
    }

    /// Recover every connected-epoch owner for explicit station teardown.
    ///
    /// This is intentionally a consuming operation: hardware, RX, TX and
    /// control may only be separated after the outer runner has stopped
    /// scheduling them.
    pub fn into_parts(self) -> (H, R, X, C) {
        let (rx, tx, control) = self.role.into_parts();
        (self.hardware, rx, tx, control)
    }
}

impl<H, R, X, C, SoftwareFrame, PhysicalFrame> DatapathServices<SoftwareFrame, PhysicalFrame>
    for SingleRoleServices<H, R, X, C>
where
    SoftwareFrame: SoftwareTxFrame,
    PhysicalFrame: crate::datapath::MaterializedTxFrame,
    R: DatapathRxService<H>,
    X: DatapathNetworkTxService<H, SoftwareFrame, PhysicalFrame>,
    C: DatapathControlService<H, X>,
{
    type Error = DatapathServiceError<R::Error, C::Error, X::Error>;
    type Exit = C::Exit;

    fn service_rx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn crate::datapath::network::DatapathNetworkRxSet,
        context: crate::datapath::DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            self.role
                .rx
                .service_turn(&mut self.hardware, network_rx, context)
                .await
                .map_err(DatapathServiceError::Rx)
        }
    }

    fn service_rx_during_tx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn crate::datapath::network::DatapathNetworkRxSet,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            self.role
                .rx
                .service_turn_during_tx(&mut self.hardware, network_rx)
                .await
                .map_err(DatapathServiceError::Rx)
        }
    }

    fn has_rx_work(&self) -> bool {
        self.role.rx.has_work()
    }

    fn serviced_rx_frames(&self) -> u64 {
        self.role.rx.serviced_frames()
    }

    fn rx_work_counters(&self) -> DatapathRxWorkCounters {
        self.role.rx.work_counters()
    }

    fn service_control<'a>(
        &'a mut self,
        context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a {
        async move {
            self.role
                .control
                .service(&mut self.hardware, &mut self.role.tx, context)
                .await
                .map_err(DatapathServiceError::Control)
        }
    }

    fn control_ready(&self, now: oer_time::Instant) -> bool {
        self.role.control.ready(&self.role.tx, now)
    }

    fn control_required_before_network_tx(&self) -> bool {
        self.role.control.required_before_network_tx()
    }

    fn control_admits_network_tx(&self) -> bool {
        self.role.control.admits_network_tx()
    }

    fn control_required_before_stop(&self) -> bool {
        self.role.control.required_before_stop()
    }

    fn wait_control_ready<'a>(&'a mut self) -> impl Future<Output = ()> + 'a {
        async move {
            let RoleRuntime { rx, tx, control } = &mut self.role;
            let _ = embassy_futures::select::select(control.wait_ready(tx), rx.wait_ready()).await;
        }
    }

    fn start_tx<'a, I>(
        &'a mut self,
        frame: SoftwareFrame,
        network: &'a I,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a
    where
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>
            + 'a,
    {
        async move {
            self.role
                .tx
                .start(&mut self.hardware, frame, network)
                .await
                .map_err(DatapathServiceError::Tx)
        }
    }

    fn last_started_tx_frame_count(&self) -> usize {
        self.role.tx.last_started_frame_count()
    }

    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        self.role.tx.wait_deadline()
    }

    fn service_tx<'a>(
        &'a mut self,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            self.role
                .tx
                .service(&mut self.hardware, wake)
                .await
                .map_err(DatapathServiceError::Tx)
        }
    }

    fn has_prepared_tx(&self) -> bool {
        self.role.tx.has_prepared()
    }

    fn tx_batch_demand<I>(&self, _interface: NetworkInterfaceId, network: &I) -> TxBatchDemand
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        self.role.tx.batch_demand(network)
    }

    fn prepared_tx_frame_count(&self) -> usize {
        self.role.tx.prepared_frame_count()
    }

    fn prepared_tx_start_ready(&self) -> bool {
        self.role.tx.prepared_start_ready()
    }

    fn advance_prepared_tx<I>(&mut self, network: &I) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        self.role
            .tx
            .advance_prepared(&mut self.hardware, network)
            .map_err(DatapathServiceError::Tx)
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn mark_prepared_tx_scheduler_phase(
        &mut self,
        phase: PreparedTxSchedulerPhase,
        at_micros: u64,
    ) {
        self.role.tx.mark_prepared_scheduler_phase(phase, at_micros);
    }

    fn start_prepared_tx<I>(&mut self, network: &I) -> Result<WifiTxProgress, Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        self.role
            .tx
            .start_prepared(&mut self.hardware, network)
            .map_err(DatapathServiceError::Tx)
    }

    fn cancel_prepared_tx<I>(&mut self, network: &I) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        self.role
            .tx
            .cancel_prepared(network)
            .map_err(DatapathServiceError::Tx)
    }

    fn can_prepare_tx(&self) -> bool {
        self.role.tx.can_prepare()
    }

    fn prepare_tx<'a, I>(
        &'a mut self,
        frame: SoftwareFrame,
        network: &'a I,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>
            + 'a,
    {
        async move {
            self.role
                .tx
                .prepare(frame, network)
                .await
                .map_err(DatapathServiceError::Tx)
        }
    }
}
