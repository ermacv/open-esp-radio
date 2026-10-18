#![expect(
    clippy::type_complexity,
    reason = "the application root returns the statically typed controller and sole owner task together"
)]

//! ESP32-S31 local-role bindings for the physical radio supervisor.

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_radio::wifi::{
    RadioController, RadioSubsystemGeneration, StationRequest, WifiStartFailure, WifiStartReport,
    WifiSupervisorConfiguration,
};
use oer_radio_supervisor::{
    WifiActiveRoleControl, WifiActiveRoleExit, WifiRoleEpochOutcome, WifiRoleEpochRunner,
    WifiRoleFrontier, WifiStartKind, WifiSupervisorControlResources, WifiSupervisorEndpoint,
    WifiSupervisorMailbox, WifiSupervisorPrepareFailure, WifiSupervisorResponse,
    WifiSupervisorTask, drive_wifi_active_role_pinned, finish_wifi_active_role,
    prepare_wifi_supervisor,
};

use oer_esp32s31_hal::owner::MacInterruptSetup;

use oer_esp32s31_phy::PhyTargetObserver;

use oer_esp32s31_ieee80211_runtime::roles::{
    monitor::{MonitorController, MonitorStopped, MonitorTask, MonitorTaskExit},
    station::{StationAttemptRunner, StationController, StationExit, StationTask},
};

use oer_esp32s31_ieee80211_mac::{irq::MacInterruptRoute, rx::RxPhyInfo};

use oer_ieee80211_runtime::await_stack_boundary;

use oer_ieee80211_softmac::{MonitorChannelPolicy, MonitorSink};

/// Application-facing ESP32-S31 radio actor. Internal supervisor endpoints
/// never escape this value.
pub type RadioSupervisorTask<'resources, M, R> = WifiSupervisorTask<'resources, M, R>;

/// Failed ESP32-S31 actor preparation with its runner and stopped owner
/// retained for explicit board-level handling.
pub type RadioSupervisorPrepareFailure<R, S> = WifiSupervisorPrepareFailure<R, S>;

/// Prepare the hardware-free controller and sole owner-holding ESP32-S31
/// radio actor together.
pub fn prepare_esp32s31_radio_supervisor<'resources, M, R>(
    control: &'resources WifiSupervisorControlResources<M, R::Error>,
    configuration: WifiSupervisorConfiguration,
    runner: R,
    stopped: R::Stopped,
) -> Result<
    (
        RadioController<WifiSupervisorMailbox<'resources, M, R::Error>>,
        RadioSupervisorTask<'resources, M, R>,
    ),
    RadioSupervisorPrepareFailure<R, R::Stopped>,
>
where
    M: RawMutex,
    R: WifiRoleEpochRunner<M>,
{
    prepare_wifi_supervisor(control, configuration, runner, stopped)
}

/// Role-neutral stopped Wi-Fi plus independently reusable role resources.
///
/// The physical supervisor stores this aggregate only between active role
/// epochs. Starting a role consumes the corresponding resource graph together
/// with `wifi`; a clean exit must return both before this value can exist
/// again.
pub struct WifiSupervisorStopped<W, H, S, A, M> {
    pub wifi: W,
    pub physical: H,
    pub station: S,
    pub access_point: A,
    pub monitor: M,
}

impl<W, H, S, A, M> WifiSupervisorStopped<W, H, S, A, M> {
    pub const fn new(wifi: W, physical: H, station: S, access_point: A, monitor: M) -> Self {
        Self {
            wifi,
            physical,
            station,
            access_point,
            monitor,
        }
    }

    pub fn into_parts(self) -> (W, H, S, A, M) {
        (
            self.wifi,
            self.physical,
            self.station,
            self.access_point,
            self.monitor,
        )
    }
}

struct StationActiveRoleControl<'borrow, 'control, M: RawMutex> {
    inner: &'borrow mut StationController<'control, M>,
}

impl<M: RawMutex> WifiActiveRoleControl for StationActiveRoleControl<'_, '_, M> {
    fn request_stop(&mut self) {
        self.inner.request_stop();
    }
}

struct MonitorActiveRoleControl<'borrow, 'control, M: RawMutex> {
    inner: &'borrow mut MonitorController<'control, M>,
}

impl<M: RawMutex> WifiActiveRoleControl for MonitorActiveRoleControl<'_, '_, M> {
    fn request_stop(&mut self) {
        self.inner.request_stop();
    }
}

/// Drive one complete ESP32-S31 station owner locally inside the physical
/// supervisor actor.
///
/// `task` owns the live station graph, while `controller` remains beside its
/// future solely to publish cooperative commands. The returned exit carries
/// the exact hardware frontier and must be classified before a pending stop is
/// acknowledged.
pub async fn drive_esp32s31_station_role<'control, M, R, E, Reject>(
    endpoint: &mut WifiSupervisorEndpoint<'_, M, E>,
    controller: &mut StationController<'control, M>,
    task: StationTask<'control, M, R>,
    reject_while_active: Reject,
) -> WifiActiveRoleExit<StationExit<R::Owner, R, R::Error, R::Fault>>
where
    M: RawMutex + 'control,
    R: StationAttemptRunner<M>,
    Reject: FnMut(WifiStartKind) -> E,
{
    let mut task = task;
    let role = task.run();
    let mut role = core::pin::pin!(role);
    let mut control = StationActiveRoleControl { inner: controller };
    await_stack_boundary!(drive_wifi_active_role_pinned(
        endpoint,
        &mut control,
        role.as_mut(),
        reject_while_active,
    ))
}

/// Owned input for one station role epoch.
pub struct StationSupervisorEpoch<S> {
    stopped: S,
    request: StationRequest,
    generation: RadioSubsystemGeneration,
}

impl<S> StationSupervisorEpoch<S> {
    pub const fn new(
        stopped: S,
        request: StationRequest,
        generation: RadioSubsystemGeneration,
    ) -> Self {
        Self {
            stopped,
            request,
            generation,
        }
    }
}

/// Board-specific classification and public-error hooks around the common
/// owner-holding station supervisor protocol.
pub struct StationSupervisorHooks<C, R, F> {
    classify: C,
    reject_while_active: R,
    fault_error: F,
}

impl<C, R, F> StationSupervisorHooks<C, R, F> {
    pub const fn new(classify: C, reject_while_active: R, fault_error: F) -> Self {
        Self {
            classify,
            reject_while_active,
            fault_error,
        }
    }
}

/// Run one complete ESP32-S31 station epoch inside the physical supervisor.
///
/// The board binding prepares the concrete station owner and classifies its
/// terminal frontier. This function acknowledges start only after task
/// preparation, keeps the controller beside the owner future, and
/// acknowledges stop only after the returned owner is classified reusable.
pub async fn run_esp32s31_station_supervisor_epoch<
    'control,
    M,
    R,
    S,
    F,
    E,
    Prepare,
    Classify,
    RejectActive,
    FaultError,
>(
    endpoint: &mut WifiSupervisorEndpoint<'_, M, E>,
    epoch: StationSupervisorEpoch<S>,
    prepare: Prepare,
    hooks: StationSupervisorHooks<Classify, RejectActive, FaultError>,
) -> WifiRoleEpochOutcome<S, F>
where
    M: RawMutex + 'control,
    R: StationAttemptRunner<M> + 'control,
    Prepare: FnOnce(
        S,
        StationRequest,
    )
        -> Result<(StationController<'control, M>, StationTask<'control, M, R>), F>,
    Classify: FnOnce(StationExit<R::Owner, R, R::Error, R::Fault>) -> WifiRoleFrontier<S, F>,
    RejectActive: FnMut(WifiStartKind) -> E,
    FaultError: FnMut(&F) -> E,
{
    let StationSupervisorEpoch {
        stopped,
        request,
        generation,
    } = epoch;
    let StationSupervisorHooks {
        classify,
        reject_while_active,
        mut fault_error,
    } = hooks;

    let (mut controller, task) = match prepare(stopped, request) {
        Ok(prepared) => prepared,
        Err(faulted) => {
            let error = fault_error(&faulted);
            endpoint
                .respond(WifiSupervisorResponse::Station(Err(
                    WifiStartFailure::faulted(error),
                )))
                .await;
            return WifiRoleEpochOutcome::Faulted(faulted);
        }
    };
    endpoint
        .respond(WifiSupervisorResponse::Station(Ok(WifiStartReport::new(
            generation,
        ))))
        .await;

    let exit = await_stack_boundary!(drive_esp32s31_station_role(
        endpoint,
        &mut controller,
        task,
        reject_while_active,
    ));
    match await_stack_boundary!(finish_wifi_active_role(
        endpoint,
        generation,
        exit,
        classify,
        fault_error,
    )) {
        WifiRoleFrontier::Stopped(stopped) => WifiRoleEpochOutcome::Stopped(stopped),
        WifiRoleFrontier::Faulted(faulted) => WifiRoleEpochOutcome::Faulted(faulted),
    }
}

/// Drive one complete standalone-monitor owner locally inside the physical
/// supervisor actor.
///
/// The returned exit preserves the distinction between a reusable stopped
/// owner and a task which still retains a quarantined live frontier. A pending
/// application stop may be acknowledged only for the former.
#[allow(clippy::type_complexity)]
pub async fn drive_esp32s31_monitor_role<
    'runtime,
    P,
    Membership,
    R,
    M,
    S,
    E,
    Reject,
    D,
    O,
    RP,
    RC,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>(
    endpoint: &mut WifiSupervisorEndpoint<'_, M, E>,
    controller: &mut MonitorController<'runtime, M>,
    radio: &oer_esp32s31_radio_runtime::RadioSystem<RP, RC, D>,
    task: MonitorTask<'runtime, P, Membership, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    channel_policy: MonitorChannelPolicy,
    observer: &mut O,
    reject_while_active: Reject,
) -> WifiActiveRoleExit<
    MonitorTaskExit<
        MonitorStopped<'runtime, P, Membership, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        MonitorTask<'runtime, P, Membership, R, M, S, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        R::Error,
    >,
>
where
    P: Sized,
    R: MacInterruptRoute<Platform = P, Setup = MacInterruptSetup>,
    M: RawMutex,
    S: MonitorSink<RxPhyInfo>,
    Reject: FnMut(WifiStartKind) -> E,
    D: oer_time::Timer,
    O: PhyTargetObserver,
    RC: oer_esp32s31_hal::shared_radio::PlatformClockProvider,
{
    let role = task.run_channel_policy_to_exit::<D, O, RP, RC>(radio, channel_policy, observer);
    let mut role = core::pin::pin!(role);
    let mut control = MonitorActiveRoleControl { inner: controller };
    await_stack_boundary!(drive_wifi_active_role_pinned(
        endpoint,
        &mut control,
        role.as_mut(),
        reject_while_active,
    ))
}
