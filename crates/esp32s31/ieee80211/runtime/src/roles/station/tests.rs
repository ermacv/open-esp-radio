#![expect(
    clippy::manual_async_fn,
    reason = "station test doubles implement the production borrowed Future contracts"
)]

use oer_time_virtual::SkipClock;

use core::{
    future::{Future, pending, ready},
    task::{Context, Poll, Waker},
};

use embassy_futures::{block_on, join::join};

use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use oer_ieee80211_sta::station::{
    StaAttemptContext, StaAttemptFailure, StaAttemptOutcome, StaFailureDisposition,
    StaLifecycleStage, StaReconnectPolicy,
};

use super::{
    connected::{
        ConnectedStationExit, StationReconnectSource, coalesce_disconnected_station_command,
        complete_connected_station_command, complete_esp32s31_connected_datapath_exit,
    },
    *,
};

struct Backend {
    fail: bool,
    clock: SkipClock,
}

struct PendingBackend {
    clock: SkipClock,
}

impl StationAttemptRunner<NoopRawMutex> for PendingBackend {
    type Owner = u32;
    type Error = u8;
    type Fault = core::convert::Infallible;
    type Timer = SkipClock;

    fn timer(&self) -> &SkipClock {
        &self.clock
    }

    fn run_attempt<'a>(
        &'a mut self,
        owner: Self::Owner,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error>> + 'a {
        async move {
            pending::<()>().await;
            StaAttemptOutcome::Stopped { owner }
        }
    }
}

impl StationAttemptRunner<NoopRawMutex> for Backend {
    type Owner = u32;
    type Error = u8;
    type Fault = core::convert::Infallible;
    type Timer = SkipClock;

    fn timer(&self) -> &SkipClock {
        &self.clock
    }

    fn run_attempt<'a>(
        &'a mut self,
        owner: Self::Owner,
        _context: StaAttemptContext,
        _control: &'a mut StationCommandReceiver<'_, NoopRawMutex>,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error>> + 'a {
        let outcome = if self.fail {
            StaAttemptOutcome::Failed {
                owner,
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::Authentication,
                    StaFailureDisposition::RetryCurrentCandidate,
                    9,
                ),
            }
        } else {
            StaAttemptOutcome::Stopped { owner }
        };
        ready(outcome)
    }
}

fn policy(attempt_limit: u16) -> StaReconnectPolicy {
    StaReconnectPolicy::new(attempt_limit, 1, 1, 1).unwrap()
}

#[test]
fn command_priority_never_downgrades_a_pending_stop() {
    let resources = StationControlResources::<NoopRawMutex>::new();
    let (controller, mut receiver) = resources.split().unwrap();
    assert!(controller.request_reconnect());
    assert!(controller.request_disconnect());
    assert!(!controller.request_reconnect());
    assert!(controller.request_stop());
    assert!(!controller.request_disconnect());
    assert_eq!(block_on(receiver.wait()), StationCommand::Stop);
}

#[test]
fn peer_disconnect_coalesces_a_pending_reconnect_without_leaking_it() {
    let resources = StationControlResources::<NoopRawMutex>::new();
    let (controller, mut receiver) = resources.split().unwrap();
    assert!(controller.request_reconnect());
    assert!(matches!(
        coalesce_disconnected_station_command::<(), _>(
            &mut receiver,
            oer_esp32s31_ieee80211_sta::connected_control::ConnectedDisconnectReason::BeaconLoss,
        ),
        ConnectedStationExit::ReconnectRequested {
            source: StationReconnectSource::CoalescedDisconnect,
        }
    ));
    assert_eq!(receiver.try_take(), None);
}

#[test]
fn terminal_connected_command_records_the_public_stop_reason() {
    let resources = StationControlResources::<NoopRawMutex>::new();
    let (_controller, mut receiver) = resources.split().unwrap();
    assert!(matches!(
        complete_connected_station_command::<(), _>(StationCommand::Stop, &mut receiver,),
        ConnectedStationExit::StationStopped(StationCommand::Stop)
    ));
    assert_eq!(receiver.take_terminal(), Some(StationCommand::Stop));
}

#[test]
fn detached_datapath_exit_coalesces_an_already_consumed_reconnect() {
    let resources = StationControlResources::<NoopRawMutex>::new();
    let (_controller, mut receiver) = resources.split().unwrap();
    let exit = complete_esp32s31_connected_datapath_exit::<(), _>(
        &mut receiver,
        Ok(crate::datapath::DatapathRunnerExit::Role(
            oer_esp32s31_ieee80211_sta::connected_control::ConnectedDisconnectReason::BeaconLoss,
        )),
        Some(StationCommand::Reconnect),
    );
    assert!(matches!(
        exit,
        ConnectedStationExit::ReconnectRequested {
            source: StationReconnectSource::CoalescedDisconnect,
        }
    ));
}

#[test]
fn detached_datapath_exit_records_an_already_consumed_stop() {
    let resources = StationControlResources::<NoopRawMutex>::new();
    let (_controller, mut receiver) = resources.split().unwrap();
    let exit = complete_esp32s31_connected_datapath_exit::<(), _>(
        &mut receiver,
        Ok(crate::datapath::DatapathRunnerExit::Role(
            oer_esp32s31_ieee80211_sta::connected_control::ConnectedDisconnectReason::BeaconLoss,
        )),
        Some(StationCommand::Stop),
    );
    assert!(matches!(
        exit,
        ConnectedStationExit::StationStopped(StationCommand::Stop)
    ));
    assert_eq!(receiver.take_terminal(), Some(StationCommand::Stop));
}

#[test]
fn controller_stop_returns_the_exact_owner_and_reason() {
    let control = StationControlResources::<NoopRawMutex>::new();
    let (mut controller, mut runner) = prepare_esp32s31_station_task(
        StationConfiguration::new(policy(2)),
        StationStartResources::new(41),
        &control,
        Backend {
            fail: false,
            clock: SkipClock::new(),
        },
    )
    .unwrap();
    let (completion, exit) = block_on(join(controller.stop(), runner.run()));
    let StationExit::Stopped {
        resources,
        progress,
        reason,
    } = exit
    else {
        panic!("station did not stop");
    };
    assert_eq!(completion, StationCompletion::Stopped);
    let (owner, runner) = resources.into_parts();
    assert_eq!(owner, 41);
    assert!(!runner.fail);
    assert_eq!(progress.attempts_started, 1);
    assert_eq!(reason, StationStopReason::Requested(StationCommand::Stop));
}

#[test]
fn retry_exhaustion_preserves_failure_and_owner() {
    let control = StationControlResources::<NoopRawMutex>::new();
    let (mut controller, mut runner) = prepare_esp32s31_station_task(
        StationConfiguration::new(policy(1)),
        StationStartResources::new(77),
        &control,
        Backend {
            fail: true,
            clock: SkipClock::new(),
        },
    )
    .unwrap();
    let exit = block_on(runner.run());
    assert_eq!(
        block_on(controller.wait_completion()),
        StationCompletion::Ended
    );
    let StationExit::RetryExhausted {
        resources,
        progress,
        failure,
    } = exit
    else {
        panic!("station did not exhaust its bounded retry policy");
    };
    let (owner, runner) = resources.into_parts();
    assert_eq!(owner, 77);
    assert!(runner.fail);
    assert_eq!(progress.attempts_started, 1);
    assert_eq!(failure.stage, StaLifecycleStage::Authentication);
    assert_eq!(failure.error, 9);
}

#[test]
fn cancelled_station_task_reports_fault_instead_of_claiming_quiescence() {
    let control = StationControlResources::<NoopRawMutex>::new();
    let (mut controller, mut task) = prepare_esp32s31_station_task(
        StationConfiguration::new(policy(1)),
        StationStartResources::new(99),
        &control,
        PendingBackend {
            clock: SkipClock::new(),
        },
    )
    .unwrap();
    let mut run = std::boxed::Box::pin(task.run());
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
    drop(run);

    assert_eq!(
        block_on(controller.wait_completion()),
        StationCompletion::Faulted
    );
    drop(controller);
    assert!(matches!(control.split(), Err(StationControlError::Faulted)));
}

#[test]
fn clean_station_completion_allows_a_later_epoch() {
    let control = StationControlResources::<NoopRawMutex>::new();
    let (mut controller, mut task) = prepare_esp32s31_station_task(
        StationConfiguration::new(policy(1)),
        StationStartResources::new(17),
        &control,
        Backend {
            fail: false,
            clock: SkipClock::new(),
        },
    )
    .unwrap();
    let (completion, _) = block_on(join(controller.stop(), task.run()));
    assert_eq!(completion, StationCompletion::Stopped);
    drop(controller);

    assert!(control.split().is_ok());
}

#[test]
fn later_epoch_waits_for_both_previous_control_endpoints_to_drop() {
    let control = StationControlResources::<NoopRawMutex>::new();
    let (controller, mut task) = prepare_esp32s31_station_task(
        StationConfiguration::new(policy(1)),
        StationStartResources::new(23),
        &control,
        Backend {
            fail: false,
            clock: SkipClock::new(),
        },
    )
    .unwrap();

    assert!(matches!(control.split(), Err(StationControlError::InUse)));
    let _exit = block_on(task.run());
    assert!(matches!(control.split(), Err(StationControlError::InUse)));
    drop(controller);
    assert!(control.split().is_ok());
}

#[test]
fn failed_task_prepare_returns_owner_policy_and_runner() {
    let control = StationControlResources::<NoopRawMutex>::new();
    let (_occupied_controller, _occupied_receiver) = control.split().unwrap();
    let config = StationConfiguration::new(policy(2));
    let failure = match prepare_esp32s31_station_task(
        config,
        StationStartResources::new(29),
        &control,
        Backend {
            fail: true,
            clock: SkipClock::new(),
        },
    ) {
        Ok(_) => panic!("an occupied command domain must reject another station task"),
        Err(failure) => failure,
    };
    let (error, returned_config, resources, runner) = failure.into_parts();
    assert_eq!(error, StationControlError::InUse);
    assert_eq!(returned_config, config);
    assert_eq!(resources.into_owner(), 29);
    assert!(runner.fail);
}
