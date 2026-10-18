#![expect(
    clippy::manual_async_fn,
    reason = "paired service implementations keep the role-neutral borrowed Future contracts explicit"
)]
#![expect(
    clippy::too_many_arguments,
    reason = "the paired boundary exposes independent physical, role, network, and control owners"
)]

//! One physical DATAPATH services owner shared by two logical interfaces.

use core::future::{Future, pending};

#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::aggregate_tx::PreparedTxSchedulerPhase;
use oer_network_interface::NetworkInterfaceId;

use super::{
    DatapathControlContext, DatapathControlProgress, DatapathRxProgress, DatapathRxServiceContext,
    DatapathRxWorkCounters, DatapathServices, DatapathStopProgress, SelectedBurstMaterializer,
    SoftwareTxFrame, TxBatchDemand, WifiTxProgress, WifiTxWake,
    network::{DatapathNetworkRx, DatapathNetworkRxSet},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathPairRole {
    First,
    Second,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathPairedPhysicalTxError {
    AlreadyLent(DatapathPairRole),
    NotLent,
    WrongRole {
        expected: DatapathPairRole,
        actual: DatapathPairRole,
    },
}

enum DatapathPairedPhysicalTxState<Ordinary, Aggregate> {
    Available {
        ordinary: Ordinary,
        aggregate: Aggregate,
    },
    Lent(DatapathPairRole),
}

/// Sole dynamic owner of the ordinary and aggregate physical TX resources.
///
/// A role may materialize its local protocol state only after taking both
/// resources together. It must return the same pair at the terminal TX edge;
/// another role cannot observe an available capability in the meantime.
pub struct DatapathPairedPhysicalTx<Ordinary, Aggregate> {
    state: DatapathPairedPhysicalTxState<Ordinary, Aggregate>,
}

enum DatapathPairedRoleState<Active, Parked> {
    Active(Active),
    Parked(Parked),
    Transitioning,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathPairedRoleTransitionError<E> {
    AlreadyActive,
    AlreadyParked,
    Conversion(E),
}

/// Role-local state whose active form temporarily contains physical TX.
///
/// The internal transition sentinel is never observable. A failed conversion
/// must return the exact input state, preventing `Option::take` holes from
/// becoming a second, implicit ownership protocol.
pub struct DatapathPairedRoleOwner<Active, Parked> {
    state: DatapathPairedRoleState<Active, Parked>,
}

impl<Active, Parked> DatapathPairedRoleOwner<Active, Parked> {
    pub const fn from_active(active: Active) -> Self {
        Self {
            state: DatapathPairedRoleState::Active(active),
        }
    }

    pub const fn parked(parked: Parked) -> Self {
        Self {
            state: DatapathPairedRoleState::Parked(parked),
        }
    }

    pub fn active(&self) -> Option<&Active> {
        match &self.state {
            DatapathPairedRoleState::Active(active) => Some(active),
            DatapathPairedRoleState::Parked(_) => None,
            DatapathPairedRoleState::Transitioning => {
                unreachable!("role transition cannot escape its owner method")
            }
        }
    }

    pub fn active_mut(&mut self) -> Option<&mut Active> {
        match &mut self.state {
            DatapathPairedRoleState::Active(active) => Some(active),
            DatapathPairedRoleState::Parked(_) => None,
            DatapathPairedRoleState::Transitioning => {
                unreachable!("role transition cannot escape its owner method")
            }
        }
    }

    pub fn parked_state(&self) -> Option<&Parked> {
        match &self.state {
            DatapathPairedRoleState::Active(_) => None,
            DatapathPairedRoleState::Parked(parked) => Some(parked),
            DatapathPairedRoleState::Transitioning => {
                unreachable!("role transition cannot escape its owner method")
            }
        }
    }

    pub fn parked_state_mut(&mut self) -> Option<&mut Parked> {
        match &mut self.state {
            DatapathPairedRoleState::Active(_) => None,
            DatapathPairedRoleState::Parked(parked) => Some(parked),
            DatapathPairedRoleState::Transitioning => {
                unreachable!("role transition cannot escape its owner method")
            }
        }
    }

    pub fn is_parked(&self) -> bool {
        matches!(self.state, DatapathPairedRoleState::Parked(_))
    }

    pub fn try_activate<E>(
        &mut self,
        activate: impl FnOnce(Parked) -> Result<Active, (E, Parked)>,
    ) -> Result<(), DatapathPairedRoleTransitionError<E>> {
        let state = core::mem::replace(&mut self.state, DatapathPairedRoleState::Transitioning);
        let DatapathPairedRoleState::Parked(parked) = state else {
            self.state = state;
            return Err(DatapathPairedRoleTransitionError::AlreadyActive);
        };
        match activate(parked) {
            Ok(active) => {
                self.state = DatapathPairedRoleState::Active(active);
                Ok(())
            }
            Err((error, parked)) => {
                self.state = DatapathPairedRoleState::Parked(parked);
                Err(DatapathPairedRoleTransitionError::Conversion(error))
            }
        }
    }

    pub fn try_park<E>(
        &mut self,
        park: impl FnOnce(Active) -> Result<Parked, (E, Active)>,
    ) -> Result<(), DatapathPairedRoleTransitionError<E>> {
        let state = core::mem::replace(&mut self.state, DatapathPairedRoleState::Transitioning);
        let DatapathPairedRoleState::Active(active) = state else {
            self.state = state;
            return Err(DatapathPairedRoleTransitionError::AlreadyParked);
        };
        match park(active) {
            Ok(parked) => {
                self.state = DatapathPairedRoleState::Parked(parked);
                Ok(())
            }
            Err((error, active)) => {
                self.state = DatapathPairedRoleState::Active(active);
                Err(DatapathPairedRoleTransitionError::Conversion(error))
            }
        }
    }

    /// Consume a quiescent role while retaining the complete owner on error.
    ///
    /// Returning only the active payload would discard the role-state
    /// boundary and make a higher-level rollback reconstruct it.  Paired
    /// teardown therefore either receives the parked payload or the exact
    /// original owner.
    pub fn try_into_parked(self) -> Result<Parked, Self> {
        match self.state {
            DatapathPairedRoleState::Parked(parked) => Ok(parked),
            DatapathPairedRoleState::Active(active) => Err(Self {
                state: DatapathPairedRoleState::Active(active),
            }),
            DatapathPairedRoleState::Transitioning => {
                unreachable!("role transition cannot escape its owner method")
            }
        }
    }

    /// Consume an active role while retaining the complete owner when it is
    /// already parked.
    pub fn try_into_active(self) -> Result<Active, Self> {
        match self.state {
            DatapathPairedRoleState::Active(active) => Ok(active),
            DatapathPairedRoleState::Parked(parked) => Err(Self {
                state: DatapathPairedRoleState::Parked(parked),
            }),
            DatapathPairedRoleState::Transitioning => {
                unreachable!("role transition cannot escape its owner method")
            }
        }
    }
}

impl<Ordinary, Aggregate> DatapathPairedPhysicalTx<Ordinary, Aggregate> {
    pub const fn new(ordinary: Ordinary, aggregate: Aggregate) -> Self {
        Self {
            state: DatapathPairedPhysicalTxState::Available {
                ordinary,
                aggregate,
            },
        }
    }

    pub const fn lent_to(&self) -> Option<DatapathPairRole> {
        match &self.state {
            DatapathPairedPhysicalTxState::Available { .. } => None,
            DatapathPairedPhysicalTxState::Lent(role) => Some(*role),
        }
    }

    pub fn try_lend(
        &mut self,
        role: DatapathPairRole,
    ) -> Result<(Ordinary, Aggregate), DatapathPairedPhysicalTxError> {
        let DatapathPairedPhysicalTxState::Available { .. } = &self.state else {
            return Err(DatapathPairedPhysicalTxError::AlreadyLent(
                self.lent_to().expect("lent state carries its role"),
            ));
        };
        let state = core::mem::replace(&mut self.state, DatapathPairedPhysicalTxState::Lent(role));
        match state {
            DatapathPairedPhysicalTxState::Available {
                ordinary,
                aggregate,
            } => Ok((ordinary, aggregate)),
            DatapathPairedPhysicalTxState::Lent(_) => unreachable!("availability checked above"),
        }
    }

    pub fn restore(
        &mut self,
        role: DatapathPairRole,
        ordinary: Ordinary,
        aggregate: Aggregate,
    ) -> Result<(), (DatapathPairedPhysicalTxError, Ordinary, Aggregate)> {
        match &self.state {
            DatapathPairedPhysicalTxState::Available { .. } => {
                Err((DatapathPairedPhysicalTxError::NotLent, ordinary, aggregate))
            }
            DatapathPairedPhysicalTxState::Lent(actual) if *actual != role => Err((
                DatapathPairedPhysicalTxError::WrongRole {
                    expected: *actual,
                    actual: role,
                },
                ordinary,
                aggregate,
            )),
            DatapathPairedPhysicalTxState::Lent(_) => {
                self.state = DatapathPairedPhysicalTxState::Available {
                    ordinary,
                    aggregate,
                };
                Ok(())
            }
        }
    }

    /// Consume an available physical pair while retaining it unchanged when
    /// a role still owns the finite hardware transaction.
    pub fn try_into_resources(self) -> Result<(Ordinary, Aggregate), Self> {
        match self.state {
            DatapathPairedPhysicalTxState::Available {
                ordinary,
                aggregate,
            } => Ok((ordinary, aggregate)),
            DatapathPairedPhysicalTxState::Lent(role) => Err(Self {
                state: DatapathPairedPhysicalTxState::Lent(role),
            }),
        }
    }
}

#[cfg(test)]
mod physical_tx_tests;

/// The role retaining software-owned TX, always derived from the roles
/// themselves: a role may retain owners inside its own start or
/// classification, so no cached copy could stay exact. At most one role
/// retains, because the runner serves a retaining VIF before any other
/// network work.
fn retained_role(first: bool, second: bool) -> Option<DatapathPairRole> {
    match (first, second) {
        (false, false) => None,
        (true, false) => Some(DatapathPairRole::First),
        (false, true) => Some(DatapathPairRole::Second),
        (true, true) => panic!("both VIFs cannot retain prepared TX ownership"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathPairedControlProgress<E> {
    Idle,
    More,
    TxPending(DatapathPairRole),
    Exit(E),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathPairedRxProgress {
    Rx(DatapathRxProgress),
    TxPending(DatapathPairRole),
}

impl From<DatapathRxProgress> for DatapathPairedRxProgress {
    fn from(progress: DatapathRxProgress) -> Self {
        Self::Rx(progress)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathPairedStopProgress {
    More,
    TxPending(DatapathPairRole),
    Stopped,
}

/// Failure at the paired DATAPATH ownership boundary.
///
/// Role-local error vocabularies remain owned by their implementations. The
/// composition records which unique service produced the failure instead of
/// requiring unrelated DMA, STA, AP and control code to share an artificial
/// common error type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatapathPairedServiceError<RxError, FirstTxError, SecondTxError, ControlError> {
    Rx(RxError),
    FirstTx(FirstTxError),
    SecondTx(SecondTxError),
    Control(ControlError),
}

/// Common RX owner for a two-interface DATAPATH composition.
///
/// Implementations own the sole physical DMA producer and ordered VIF
/// dispatcher. The two publication arguments are already narrowed logical
/// endpoints; neither role processor can publish through the other one.
pub trait DatapathPairedRxService<H, PhysicalTx, FirstRole, SecondRole> {
    type Error: 'static;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        first_role: &'a mut FirstRole,
        second_role: &'a mut SecondRole,
        first: &'a mut dyn DatapathNetworkRx,
        second: &'a mut dyn DatapathNetworkRx,
        context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathPairedRxProgress, Self::Error>> + 'a;

    fn service_during_tx<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        first_role: &'a mut FirstRole,
        second_role: &'a mut SecondRole,
        first: &'a mut dyn DatapathNetworkRx,
        second: &'a mut dyn DatapathNetworkRx,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a;

    fn can_service_during_tx(&self) -> bool {
        true
    }

    fn has_work(&self, _first_role: &FirstRole, _second_role: &SecondRole) -> bool {
        false
    }

    fn serviced_frames(&self) -> u64 {
        0
    }

    /// Monotonic physical units and bytes completed by the shared RX DMA
    /// producer.
    ///
    /// This has no default deliberately: a paired physical RX owner must not
    /// silently disable the role-neutral adaptive continuation policy.
    fn work_counters(&self) -> DatapathRxWorkCounters;
}

/// Control/lifecycle arbiter for two roles sharing one hardware transaction.
///
/// Every pending TX carries its role explicitly. The outer DATAPATH uses that
/// identity to narrow completion, deadline and standby ownership.
pub trait DatapathPairedControlService<H, PhysicalTx, FirstTx, SecondTx> {
    type Error: 'static;
    type Exit: 'static;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        first_tx: &'a mut FirstTx,
        second_tx: &'a mut SecondTx,
        context: DatapathControlContext,
        retained_tx: Option<DatapathPairRole>,
    ) -> impl Future<Output = Result<DatapathPairedControlProgress<Self::Exit>, Self::Error>> + 'a;

    /// O(1) readiness observation which borrows no physical TX capability.
    fn ready(&self, _first_tx: &FirstTx, _second_tx: &SecondTx, _now: oer_time::Instant) -> bool {
        true
    }

    /// Whether a controlled stop must run control before [`Self::stop`].
    fn required_before_stop(&self, _first_tx: &FirstTx, _second_tx: &SecondTx) -> bool {
        false
    }

    fn stop(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut PhysicalTx,
        first_tx: &mut FirstTx,
        second_tx: &mut SecondTx,
    ) -> Result<DatapathPairedStopProgress, Self::Error>;

    fn wait_ready<'a>(
        &'a mut self,
        physical_tx: &'a mut PhysicalTx,
        first_tx: &'a mut FirstTx,
        second_tx: &'a mut SecondTx,
    ) -> impl Future<Output = ()> + 'a;
}

/// Unique paired-epoch owner of hardware, common RX, two role TX services and
/// their typed control arbiter.
pub trait DatapathPairedNetworkTxService<H, PhysicalTx, SoftwareFrame, PhysicalFrame>
where
    SoftwareFrame: SoftwareTxFrame,
    PhysicalFrame: crate::datapath::MaterializedTxFrame,
{
    type Error: 'static;

    fn start<'a, I>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        frame: SoftwareFrame,
        network: &'a I,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a
    where
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>
            + 'a;

    fn last_started_frame_count(&self) -> usize {
        1
    }

    fn wait_deadline<'a>(
        &'a mut self,
        physical_tx: &'a mut PhysicalTx,
    ) -> impl Future<Output = ()> + 'a;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical_tx: &'a mut PhysicalTx,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a;

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

    fn advance_prepared<I>(
        &mut self,
        _hardware: &mut H,
        _physical_tx: &mut PhysicalTx,
        _network: &I,
    ) -> Result<(), Self::Error>
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
        _physical_tx: &mut PhysicalTx,
        _network: &I,
    ) -> Result<WifiTxProgress, Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        Ok(WifiTxProgress::Complete)
    }

    fn cancel_prepared<I>(
        &mut self,
        _physical_tx: &mut PhysicalTx,
        _network: &I,
    ) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        Ok(())
    }

    fn can_prepare(&self, _physical_tx: &PhysicalTx) -> bool {
        false
    }

    fn prepare<'a, I>(
        &'a mut self,
        _physical_tx: &'a mut PhysicalTx,
        _frame: SoftwareFrame,
        _network: &'a I,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        H: 'a,
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>
            + 'a,
    {
        core::future::ready(Ok(()))
    }
}

pub struct ConcurrentRoleServices<H, PhysicalTx, R, FirstTx, SecondTx, C> {
    first_interface: NetworkInterfaceId,
    second_interface: NetworkInterfaceId,
    hardware: H,
    physical_tx: PhysicalTx,
    rx: R,
    first_tx: FirstTx,
    second_tx: SecondTx,
    control: C,
    active: Option<DatapathPairRole>,
    last_started_frames: usize,
}

impl<H, PhysicalTx, R, FirstTx, SecondTx, C>
    ConcurrentRoleServices<H, PhysicalTx, R, FirstTx, SecondTx, C>
{
    pub fn new(
        first_interface: NetworkInterfaceId,
        second_interface: NetworkInterfaceId,
        hardware: H,
        physical_tx: PhysicalTx,
        rx: R,
        first_tx: FirstTx,
        second_tx: SecondTx,
        control: C,
    ) -> Self {
        assert_ne!(
            first_interface, second_interface,
            "paired DATAPATH services require distinct interfaces"
        );
        Self {
            first_interface,
            second_interface,
            hardware,
            physical_tx,
            rx,
            first_tx,
            second_tx,
            control,
            active: None,
            last_started_frames: 1,
        }
    }

    pub const fn hardware(&self) -> &H {
        &self.hardware
    }

    pub fn hardware_mut(&mut self) -> &mut H {
        &mut self.hardware
    }

    pub const fn rx(&self) -> &R {
        &self.rx
    }

    pub fn rx_mut(&mut self) -> &mut R {
        &mut self.rx
    }

    /// Borrow the sole hardware and common RX owners together.
    ///
    /// A same-channel composition starts and stops its one DMA producer at
    /// the outer paired boundary. Returning two disjoint borrows prevents a
    /// caller from extracting either owner or manufacturing a second RX
    /// service merely to satisfy borrow checking.
    pub fn hardware_and_rx_mut(&mut self) -> (&mut H, &mut R) {
        (&mut self.hardware, &mut self.rx)
    }

    pub fn into_parts(self) -> (H, PhysicalTx, R, FirstTx, SecondTx, C) {
        (
            self.hardware,
            self.physical_tx,
            self.rx,
            self.first_tx,
            self.second_tx,
            self.control,
        )
    }

    fn role_for(&self, interface: NetworkInterfaceId) -> DatapathPairRole {
        if interface == self.first_interface {
            DatapathPairRole::First
        } else if interface == self.second_interface {
            DatapathPairRole::Second
        } else {
            panic!("TX interface does not belong to paired DATAPATH services")
        }
    }

    const fn interface_for(&self, role: DatapathPairRole) -> NetworkInterfaceId {
        match role {
            DatapathPairRole::First => self.first_interface,
            DatapathPairRole::Second => self.second_interface,
        }
    }
}

impl<H, PhysicalTx, R, FirstTx, SecondTx, C, SoftwareFrame, PhysicalFrame>
    DatapathServices<SoftwareFrame, PhysicalFrame>
    for ConcurrentRoleServices<H, PhysicalTx, R, FirstTx, SecondTx, C>
where
    SoftwareFrame: SoftwareTxFrame,
    PhysicalFrame: crate::datapath::MaterializedTxFrame,
    R: DatapathPairedRxService<H, PhysicalTx, FirstTx, SecondTx>,
    FirstTx: DatapathPairedNetworkTxService<H, PhysicalTx, SoftwareFrame, PhysicalFrame>,
    SecondTx: DatapathPairedNetworkTxService<H, PhysicalTx, SoftwareFrame, PhysicalFrame>,
    C: DatapathPairedControlService<H, PhysicalTx, FirstTx, SecondTx>,
{
    type Error = DatapathPairedServiceError<R::Error, FirstTx::Error, SecondTx::Error, C::Error>;
    type Exit = C::Exit;

    fn service_rx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn DatapathNetworkRxSet,
        context: DatapathRxServiceContext,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            let (first, second) = network_rx
                .pair_mut(self.first_interface, self.second_interface)
                .expect("paired DATAPATH runner must retain both addressed RX endpoints");
            let retained_tx =
                retained_role(self.first_tx.has_prepared(), self.second_tx.has_prepared());
            if retained_tx.is_some() {
                // A software standby is not an idle physical boundary.  The
                // DMA producer may continue reclaiming descriptors, but a
                // role protocol consumer could require the ordinary TX owner
                // and therefore cannot run until the retained role completes.
                return self
                    .rx
                    .service_during_tx(
                        &mut self.hardware,
                        &mut self.physical_tx,
                        &mut self.first_tx,
                        &mut self.second_tx,
                        first,
                        second,
                    )
                    .await
                    .map_err(DatapathPairedServiceError::Rx);
            }
            let progress = self
                .rx
                .service(
                    &mut self.hardware,
                    &mut self.physical_tx,
                    &mut self.first_tx,
                    &mut self.second_tx,
                    first,
                    second,
                    context,
                )
                .await
                .map_err(DatapathPairedServiceError::Rx)?;
            Ok(match progress {
                DatapathPairedRxProgress::Rx(progress) => progress,
                DatapathPairedRxProgress::TxPending(role) => {
                    self.active = Some(role);
                    DatapathRxProgress::ProbePending
                }
            })
        }
    }

    fn can_service_rx_during_tx(&self) -> bool {
        self.rx.can_service_during_tx()
    }

    fn service_rx_during_tx<'a>(
        &'a mut self,
        network_rx: &'a mut dyn DatapathNetworkRxSet,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        async move {
            let (first, second) = network_rx
                .pair_mut(self.first_interface, self.second_interface)
                .expect("paired DATAPATH runner must retain both addressed RX endpoints");
            self.rx
                .service_during_tx(
                    &mut self.hardware,
                    &mut self.physical_tx,
                    &mut self.first_tx,
                    &mut self.second_tx,
                    first,
                    second,
                )
                .await
                .map_err(DatapathPairedServiceError::Rx)
        }
    }

    fn has_rx_work(&self) -> bool {
        let retained_tx =
            retained_role(self.first_tx.has_prepared(), self.second_tx.has_prepared());
        retained_tx.is_none() && self.rx.has_work(&self.first_tx, &self.second_tx)
    }

    fn serviced_rx_frames(&self) -> u64 {
        self.rx.serviced_frames()
    }

    fn rx_work_counters(&self) -> DatapathRxWorkCounters {
        self.rx.work_counters()
    }

    fn service_control<'a>(
        &'a mut self,
        context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a {
        async move {
            let retained_tx =
                retained_role(self.first_tx.has_prepared(), self.second_tx.has_prepared());
            let progress = self
                .control
                .service(
                    &mut self.hardware,
                    &mut self.physical_tx,
                    &mut self.first_tx,
                    &mut self.second_tx,
                    context,
                    retained_tx,
                )
                .await
                .map_err(DatapathPairedServiceError::Control)?;
            Ok(match progress {
                DatapathPairedControlProgress::Idle => DatapathControlProgress::Idle,
                DatapathPairedControlProgress::More => DatapathControlProgress::More,
                DatapathPairedControlProgress::TxPending(role) => {
                    self.active = Some(role);
                    DatapathControlProgress::TxPending
                }
                DatapathPairedControlProgress::Exit(exit) => DatapathControlProgress::Exit(exit),
            })
        }
    }

    fn control_ready(&self, now: oer_time::Instant) -> bool {
        self.control.ready(&self.first_tx, &self.second_tx, now)
    }

    fn control_required_before_stop(&self) -> bool {
        self.control
            .required_before_stop(&self.first_tx, &self.second_tx)
    }

    fn active_tx_interface(&self) -> Option<NetworkInterfaceId> {
        self.active.map(|role| self.interface_for(role))
    }

    fn prepared_tx_interface(&self) -> Option<NetworkInterfaceId> {
        retained_role(self.first_tx.has_prepared(), self.second_tx.has_prepared())
            .map(|role| self.interface_for(role))
    }

    fn service_stop(&mut self) -> Result<DatapathStopProgress, Self::Error> {
        Ok(
            match self
                .control
                .stop(
                    &mut self.hardware,
                    &mut self.physical_tx,
                    &mut self.first_tx,
                    &mut self.second_tx,
                )
                .map_err(DatapathPairedServiceError::Control)?
            {
                DatapathPairedStopProgress::More => DatapathStopProgress::More,
                DatapathPairedStopProgress::TxPending(role) => {
                    self.active = Some(role);
                    DatapathStopProgress::TxPending
                }
                DatapathPairedStopProgress::Stopped => DatapathStopProgress::Stopped,
            },
        )
    }

    fn wait_control_ready<'a>(&'a mut self) -> impl Future<Output = ()> + 'a {
        self.control.wait_ready(
            &mut self.physical_tx,
            &mut self.first_tx,
            &mut self.second_tx,
        )
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
            let role = self.role_for(network.interface());
            assert_eq!(frame.interface(), network.interface());
            let progress = match role {
                DatapathPairRole::First => self
                    .first_tx
                    .start(&mut self.hardware, &mut self.physical_tx, frame, network)
                    .await
                    .map_err(DatapathPairedServiceError::FirstTx)?,
                DatapathPairRole::Second => self
                    .second_tx
                    .start(&mut self.hardware, &mut self.physical_tx, frame, network)
                    .await
                    .map_err(DatapathPairedServiceError::SecondTx)?,
            };
            self.last_started_frames = match role {
                DatapathPairRole::First => self.first_tx.last_started_frame_count(),
                DatapathPairRole::Second => self.second_tx.last_started_frame_count(),
            }
            .max(1);
            self.active = (progress == WifiTxProgress::Pending).then_some(role);
            Ok(progress)
        }
    }

    fn last_started_tx_frame_count(&self) -> usize {
        self.last_started_frames
    }

    fn wait_tx_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        async move {
            match self.active {
                Some(DatapathPairRole::First) => {
                    self.first_tx.wait_deadline(&mut self.physical_tx).await
                }
                Some(DatapathPairRole::Second) => {
                    self.second_tx.wait_deadline(&mut self.physical_tx).await
                }
                None => pending().await,
            }
        }
    }

    fn service_tx<'a>(
        &'a mut self,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let role = self.active.expect("paired TX completion requires one role");
            let progress = match role {
                DatapathPairRole::First => self
                    .first_tx
                    .service(&mut self.hardware, &mut self.physical_tx, wake)
                    .await
                    .map_err(DatapathPairedServiceError::FirstTx)?,
                DatapathPairRole::Second => self
                    .second_tx
                    .service(&mut self.hardware, &mut self.physical_tx, wake)
                    .await
                    .map_err(DatapathPairedServiceError::SecondTx)?,
            };
            if progress == WifiTxProgress::Complete {
                self.active = None;
            }
            Ok(progress)
        }
    }

    fn has_prepared_tx(&self) -> bool {
        retained_role(self.first_tx.has_prepared(), self.second_tx.has_prepared()).is_some()
    }

    fn tx_batch_demand<I>(&self, interface: NetworkInterfaceId, network: &I) -> TxBatchDemand
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        match self.role_for(interface) {
            DatapathPairRole::First => self.first_tx.batch_demand(network),
            DatapathPairRole::Second => self.second_tx.batch_demand(network),
        }
    }

    fn prepared_tx_frame_count(&self) -> usize {
        match retained_role(self.first_tx.has_prepared(), self.second_tx.has_prepared()) {
            Some(DatapathPairRole::First) => self.first_tx.prepared_frame_count(),
            Some(DatapathPairRole::Second) => self.second_tx.prepared_frame_count(),
            None => 0,
        }
    }

    fn prepared_tx_start_ready(&self) -> bool {
        match retained_role(self.first_tx.has_prepared(), self.second_tx.has_prepared()) {
            Some(DatapathPairRole::First) => self.first_tx.prepared_start_ready(),
            Some(DatapathPairRole::Second) => self.second_tx.prepared_start_ready(),
            None => false,
        }
    }

    fn advance_prepared_tx<I>(&mut self, network: &I) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        let role = retained_role(self.first_tx.has_prepared(), self.second_tx.has_prepared())
            .expect("prepared TX advancement requires one retained role");
        assert_eq!(self.interface_for(role), network.interface());
        match role {
            DatapathPairRole::First => self
                .first_tx
                .advance_prepared(&mut self.hardware, &mut self.physical_tx, network)
                .map_err(DatapathPairedServiceError::FirstTx),
            DatapathPairRole::Second => self
                .second_tx
                .advance_prepared(&mut self.hardware, &mut self.physical_tx, network)
                .map_err(DatapathPairedServiceError::SecondTx),
        }
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn mark_prepared_tx_scheduler_phase(
        &mut self,
        phase: PreparedTxSchedulerPhase,
        at_micros: u64,
    ) {
        match retained_role(self.first_tx.has_prepared(), self.second_tx.has_prepared()) {
            Some(DatapathPairRole::First) => {
                self.first_tx
                    .mark_prepared_scheduler_phase(phase, at_micros);
            }
            Some(DatapathPairRole::Second) => {
                self.second_tx
                    .mark_prepared_scheduler_phase(phase, at_micros);
            }
            None => {}
        }
    }

    fn start_prepared_tx<I>(&mut self, network: &I) -> Result<WifiTxProgress, Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        let role = retained_role(self.first_tx.has_prepared(), self.second_tx.has_prepared())
            .expect("prepared TX requires one retained role");
        assert_eq!(self.interface_for(role), network.interface());
        let progress = match role {
            DatapathPairRole::First => self
                .first_tx
                .start_prepared(&mut self.hardware, &mut self.physical_tx, network)
                .map_err(DatapathPairedServiceError::FirstTx)?,
            DatapathPairRole::Second => self
                .second_tx
                .start_prepared(&mut self.hardware, &mut self.physical_tx, network)
                .map_err(DatapathPairedServiceError::SecondTx)?,
        };
        self.active = (progress == WifiTxProgress::Pending).then_some(role);
        Ok(progress)
    }

    fn cancel_prepared_tx<I>(&mut self, network: &I) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = PhysicalFrame>,
    {
        let prepared = retained_role(self.first_tx.has_prepared(), self.second_tx.has_prepared());
        match prepared {
            Some(DatapathPairRole::First) => self
                .first_tx
                .cancel_prepared(&mut self.physical_tx, network)
                .map_err(DatapathPairedServiceError::FirstTx),
            Some(DatapathPairRole::Second) => self
                .second_tx
                .cancel_prepared(&mut self.physical_tx, network)
                .map_err(DatapathPairedServiceError::SecondTx),
            None => Ok(()),
        }
    }

    fn can_prepare_tx(&self) -> bool {
        match self.active.or(retained_role(
            self.first_tx.has_prepared(),
            self.second_tx.has_prepared(),
        )) {
            Some(DatapathPairRole::First) => self.first_tx.can_prepare(&self.physical_tx),
            Some(DatapathPairRole::Second) => self.second_tx.can_prepare(&self.physical_tx),
            None => {
                self.first_tx.can_prepare(&self.physical_tx)
                    || self.second_tx.can_prepare(&self.physical_tx)
            }
        }
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
            let role = self.role_for(network.interface());
            assert_eq!(frame.interface(), network.interface());
            if let Some(retained) =
                retained_role(self.first_tx.has_prepared(), self.second_tx.has_prepared())
            {
                assert_eq!(retained, role, "prepared aggregate cannot cross VIFs");
            }
            match role {
                DatapathPairRole::First => {
                    self.first_tx
                        .prepare(&mut self.physical_tx, frame, network)
                        .await
                        .map_err(DatapathPairedServiceError::FirstTx)?;
                }
                DatapathPairRole::Second => {
                    self.second_tx
                        .prepare(&mut self.physical_tx, frame, network)
                        .await
                        .map_err(DatapathPairedServiceError::SecondTx)?;
                }
            }
            Ok(())
        }
    }
}
