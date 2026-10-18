//! One local physical-owner actor, role epochs and fault retention.

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_ieee80211_runtime::await_stack_boundary;

use oer_radio::wifi::{
    RadioController, WifiRadioRestartReport, WifiRadioRestartRf, WifiScanFailure,
    WifiServicePlanningError, WifiServiceRequest, WifiStartFailure, WifiSupervisorConfiguration,
};

use super::dispatch::{WifiStoppedDispatch, dispatch_wifi_stopped_command};
use super::message::{WifiSupervisorCommand, WifiSupervisorResponse};
use super::transport::{
    WifiSupervisorControlError, WifiSupervisorControlResources, WifiSupervisorEndpoint,
    WifiSupervisorMailbox,
};

/// Complete result of one locally executed role epoch.
///
/// `NotStarted` means materialization rejected the request and already sent a
/// typed response, so the generation does not advance. `Stopped` means start
/// was acknowledged and the complete reusable owner returned. `Faulted`
/// retains the exact quarantined frontier and cannot re-enter stopped state.
pub enum WifiRoleEpochOutcome<S, F> {
    NotStarted(S),
    Stopped(S),
    Faulted(F),
}

/// Concrete local-role composition used by the physical supervisor.
///
/// The returned future covers the complete active epoch, including any child
/// execution in the same local ownership domain. Implementations acknowledge
/// start, drive the role alongside `endpoint`, recover the exact terminal
/// owner and complete a pending stop before returning. A controlled child must
/// return its owner through a local rendezvous before quiescence; detached
/// tasks and synchronized channels must not acquire independent live owners.
pub trait WifiRoleEpochRunner<M: RawMutex> {
    type Stopped;
    type Faulted;
    /// Fail-stop owner returned by a role-neutral whole-radio transaction.
    ///
    /// Implementations may use a compact handle to fault storage kept outside
    /// the supervisor frame. Active-role faults can retain a much larger owner
    /// graph through `Faulted` independently.
    type LifecycleFaulted;
    type Error;

    fn planning_error(&mut self, error: WifiServicePlanningError) -> Self::Error;

    /// Translate a retained hardware fault into the public control-plane
    /// error. This classifies the state; it does not perform recovery.
    fn fault_error(&mut self, faulted: &Self::Faulted) -> Self::Error;

    fn lifecycle_fault_error(&mut self, faulted: &Self::LifecycleFaulted) -> Self::Error;

    fn run_epoch<'a>(
        &'a mut self,
        endpoint: &'a mut WifiSupervisorEndpoint<'_, M, Self::Error>,
        stopped: Self::Stopped,
        service: WifiServiceRequest,
        generation: oer_radio::wifi::RadioSubsystemGeneration,
    ) -> impl Future<Output = WifiRoleEpochOutcome<Self::Stopped, Self::Faulted>> + 'a;

    /// Take Wi-Fi off the shared radio and bring it up again from the actor's
    /// stopped slot. Success must repopulate the slot; failure leaves it
    /// empty and retains the exact physical owner in `LifecycleFaulted`.
    fn restart_radio<'a>(
        &'a mut self,
        stopped: &'a mut Option<Self::Stopped>,
    ) -> impl Future<Output = Result<WifiRadioRestartRf, Self::LifecycleFaulted>> + 'a;
}

/// Prepared owner-holding supervisor actor.
///
/// The application receives only the hardware-free [`RadioController`]. This
/// value retains the physical endpoint, reusable stopped frontier and role
/// runner together, so application code cannot detach or accidentally drive
/// an internal endpoint independently of the hardware owner.
pub struct WifiSupervisorTask<'resources, M, R>
where
    M: RawMutex,
    R: WifiRoleEpochRunner<M>,
{
    endpoint: WifiSupervisorEndpoint<'resources, M, R::Error>,
    configuration: WifiSupervisorConfiguration,
    runner: R,
    stopped: R::Stopped,
}

impl<M, R> WifiSupervisorTask<'_, M, R>
where
    M: RawMutex,
    R: WifiRoleEpochRunner<M>,
{
    /// Run the sole physical radio actor forever.
    pub async fn run(self) -> ! {
        await_stack_boundary!(run_wifi_supervisor_actor(
            self.endpoint,
            self.configuration,
            self.runner,
            self.stopped,
        ))
    }
}

/// Failed preparation with every movable owner retained.
pub struct WifiSupervisorPrepareFailure<R, S> {
    configuration: WifiSupervisorConfiguration,
    runner: R,
    stopped: S,
}

impl<R, S> WifiSupervisorPrepareFailure<R, S> {
    pub const fn error(&self) -> WifiSupervisorControlError {
        WifiSupervisorControlError::InUse
    }

    pub fn into_parts(self) -> (WifiSupervisorConfiguration, R, S) {
        (self.configuration, self.runner, self.stopped)
    }
}

/// Controller and sole owner-holding actor prepared against the same resources.
pub type WifiSupervisorPrepared<'resources, M, R> = (
    RadioController<WifiSupervisorMailbox<'resources, M, <R as WifiRoleEpochRunner<M>>::Error>>,
    WifiSupervisorTask<'resources, M, R>,
);

/// Prepare the controller/actor pair as one operation.
///
/// Failure cannot lose the role runner or stopped hardware frontier.
pub fn prepare_wifi_supervisor<'resources, M, R>(
    control: &'resources WifiSupervisorControlResources<M, R::Error>,
    configuration: WifiSupervisorConfiguration,
    runner: R,
    stopped: R::Stopped,
) -> Result<WifiSupervisorPrepared<'resources, M, R>, WifiSupervisorPrepareFailure<R, R::Stopped>>
where
    M: RawMutex,
    R: WifiRoleEpochRunner<M>,
{
    let (controller, endpoint) = match control.split() {
        Ok(endpoints) => endpoints,
        Err(WifiSupervisorControlError::InUse) => {
            return Err(WifiSupervisorPrepareFailure {
                configuration,
                runner,
                stopped,
            });
        }
    };
    Ok((
        controller,
        WifiSupervisorTask {
            endpoint,
            configuration,
            runner,
            stopped,
        },
    ))
}

/// Run the physical Wi-Fi supervisor as one owner-holding local actor.
///
/// Between epochs this function owns `R::Stopped`. During an epoch that owner
/// is moved into `R::run_epoch`, whose future is awaited in this same task. A
/// reusable owner can therefore reappear only through the explicit
/// `NotStarted` or `Stopped` return variants.
pub async fn run_wifi_supervisor_actor<M, R>(
    mut endpoint: WifiSupervisorEndpoint<'_, M, R::Error>,
    configuration: WifiSupervisorConfiguration,
    mut runner: R,
    stopped: R::Stopped,
) -> !
where
    M: RawMutex,
    R: WifiRoleEpochRunner<M>,
{
    let mut generation = oer_radio::wifi::RadioSubsystemGeneration::INITIAL;
    let mut stopped = Some(stopped);
    loop {
        let service = loop {
            match dispatch_wifi_stopped_command(&mut endpoint, configuration, generation, |error| {
                runner.planning_error(error)
            })
            .await
            {
                WifiStoppedDispatch::Handled => {}
                WifiStoppedDispatch::RestartRadio => {
                    match await_stack_boundary!(runner.restart_radio(&mut stopped)) {
                        Ok(rf) => {
                            generation = generation.next();
                            endpoint
                                .respond(WifiSupervisorResponse::RestartRadio(Ok(
                                    WifiRadioRestartReport::new(generation, rf),
                                )))
                                .await;
                        }
                        Err(faulted) => {
                            endpoint
                                .respond(WifiSupervisorResponse::RestartRadio(Err(
                                    runner.lifecycle_fault_error(&faulted)
                                )))
                                .await;
                            run_wifi_lifecycle_faulted_actor(&mut endpoint, &mut runner, faulted)
                                .await
                        }
                    }
                }
                WifiStoppedDispatch::Start(service) => break service,
            }
        };

        let next_generation = generation.next();
        match await_stack_boundary!(
            runner.run_epoch(
                &mut endpoint,
                stopped
                    .take()
                    .expect("stopped owner is present between supervisor epochs"),
                service,
                next_generation
            )
        ) {
            WifiRoleEpochOutcome::NotStarted(returned) => stopped = Some(returned),
            WifiRoleEpochOutcome::Stopped(returned) => {
                stopped = Some(returned);
                generation = next_generation;
            }
            WifiRoleEpochOutcome::Faulted(faulted) => {
                run_wifi_faulted_actor(&mut endpoint, &mut runner, faulted).await
            }
        }
    }
}

async fn run_wifi_faulted_actor<M, R>(
    endpoint: &mut WifiSupervisorEndpoint<'_, M, R::Error>,
    runner: &mut R,
    faulted: R::Faulted,
) -> !
where
    M: RawMutex,
    R: WifiRoleEpochRunner<M>,
{
    loop {
        let response = match endpoint.receive().await {
            WifiSupervisorCommand::Scan(request) => {
                WifiSupervisorResponse::Scan(Err(WifiScanFailure::Rejected {
                    request,
                    error: runner.fault_error(&faulted),
                }))
            }
            WifiSupervisorCommand::StartStation(request) => WifiSupervisorResponse::Station(Err(
                WifiStartFailure::rejected(request, runner.fault_error(&faulted)),
            )),
            WifiSupervisorCommand::StartAccessPoint(request) => {
                WifiSupervisorResponse::AccessPoint(Err(WifiStartFailure::rejected(
                    request,
                    runner.fault_error(&faulted),
                )))
            }
            WifiSupervisorCommand::StartStationAccessPoint(request) => {
                WifiSupervisorResponse::StationAccessPoint(Err(WifiStartFailure::rejected(
                    request,
                    runner.fault_error(&faulted),
                )))
            }
            WifiSupervisorCommand::StartMonitor(request) => WifiSupervisorResponse::Monitor(Err(
                WifiStartFailure::rejected(request, runner.fault_error(&faulted)),
            )),
            WifiSupervisorCommand::Stop => {
                WifiSupervisorResponse::Stop(Err(runner.fault_error(&faulted)))
            }
            WifiSupervisorCommand::RestartRadio => {
                WifiSupervisorResponse::RestartRadio(Err(runner.fault_error(&faulted)))
            }
        };
        endpoint.respond(response).await;
    }
}

async fn run_wifi_lifecycle_faulted_actor<M, R>(
    endpoint: &mut WifiSupervisorEndpoint<'_, M, R::Error>,
    runner: &mut R,
    faulted: R::LifecycleFaulted,
) -> !
where
    M: RawMutex,
    R: WifiRoleEpochRunner<M>,
{
    loop {
        let response = match endpoint.receive().await {
            WifiSupervisorCommand::Scan(request) => {
                WifiSupervisorResponse::Scan(Err(WifiScanFailure::Rejected {
                    request,
                    error: runner.lifecycle_fault_error(&faulted),
                }))
            }
            WifiSupervisorCommand::StartStation(request) => WifiSupervisorResponse::Station(Err(
                WifiStartFailure::rejected(request, runner.lifecycle_fault_error(&faulted)),
            )),
            WifiSupervisorCommand::StartAccessPoint(request) => {
                WifiSupervisorResponse::AccessPoint(Err(WifiStartFailure::rejected(
                    request,
                    runner.lifecycle_fault_error(&faulted),
                )))
            }
            WifiSupervisorCommand::StartStationAccessPoint(request) => {
                WifiSupervisorResponse::StationAccessPoint(Err(WifiStartFailure::rejected(
                    request,
                    runner.lifecycle_fault_error(&faulted),
                )))
            }
            WifiSupervisorCommand::StartMonitor(request) => WifiSupervisorResponse::Monitor(Err(
                WifiStartFailure::rejected(request, runner.lifecycle_fault_error(&faulted)),
            )),
            WifiSupervisorCommand::Stop => {
                WifiSupervisorResponse::Stop(Err(runner.lifecycle_fault_error(&faulted)))
            }
            WifiSupervisorCommand::RestartRadio => {
                WifiSupervisorResponse::RestartRadio(Err(runner.lifecycle_fault_error(&faulted)))
            }
        };
        endpoint.respond(response).await;
    }
}
