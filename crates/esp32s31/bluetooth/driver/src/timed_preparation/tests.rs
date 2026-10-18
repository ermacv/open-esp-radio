use std::collections::VecDeque;

use crate::timed_preparation::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Phase(&'static str);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RollbackOwner(&'static str);

#[derive(Debug)]
struct ModelController {
    identity: &'static str,
    request: Option<Result<ControllerTimeRequest, ControllerTimeAcquisitionError>>,
    rechecks: VecDeque<Result<ControllerTimePendingOwnerStep, ControllerTimeEventError>>,
    cancel: Result<(), ControllerTimeEventError>,
    drains: VecDeque<Result<ControllerTimePendingOrphanStep, ControllerTimeEventError>>,
    restored: Option<Phase>,
    rollback_failure: Option<RollbackOwner>,
}

impl ModelController {
    fn ready(identity: &'static str) -> Self {
        Self {
            identity,
            request: Some(Ok(ControllerTimeRequest::for_validation(1))),
            rechecks: VecDeque::new(),
            cancel: Ok(()),
            drains: VecDeque::new(),
            restored: None,
            rollback_failure: None,
        }
    }
}

impl ControllerTimePendingOwner for ModelController {
    fn recheck_owned_controller_time(
        &mut self,
        _request: ControllerTimeRequest,
    ) -> Result<ControllerTimePendingOwnerStep, ControllerTimeEventError> {
        self.rechecks
            .pop_front()
            .expect("the test scripts every bounded recheck")
    }

    fn cancel_owned_controller_time(
        &mut self,
        _request: ControllerTimeRequest,
    ) -> Result<(), ControllerTimeEventError> {
        self.cancel
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<ControllerTimePendingOrphanStep, ControllerTimeEventError> {
        self.drains
            .pop_front()
            .expect("the test scripts every bounded orphan drain")
    }
}

impl TimedPreparationController for ModelController {
    fn request_timed_preparation_sample(
        &mut self,
    ) -> Result<ControllerTimeRequest, ControllerTimeAcquisitionError> {
        self.request
            .take()
            .expect("one preparation publishes exactly one request")
    }
}

fn rollback(
    controller: &mut ModelController,
    phase: Phase,
) -> TimedPreparationRollbackOutcome<RollbackOwner> {
    controller.restored = Some(phase);
    match controller.rollback_failure.take() {
        Some(owner) => TimedPreparationRollbackOutcome::FailStop(owner),
        None => TimedPreparationRollbackOutcome::Restored,
    }
}

#[test]
fn newly_issued_ready_request_needs_no_executor_wait() {
    let mut controller = ModelController::ready("immediate");
    controller
        .rechecks
        .push_back(Ok(ControllerTimePendingOwnerStep::Ready(
            ControllerTimeSample::for_validation(7),
        )));
    let pending = TimedPreparationPending::begin(controller, Phase("sequence"), rollback)
        .expect("a fresh request owns the phase");
    let TimedPreparationStep::Ready {
        controller,
        phase,
        sample,
    } = pending.recheck()
    else {
        panic!("an already-ready latch must advance on its first bounded check");
    };
    assert_eq!(controller.identity, "immediate");
    assert_eq!(phase, Phase("sequence"));
    assert_eq!(sample, ControllerTimeSample::for_validation(7));
    assert!(controller.rechecks.is_empty());
    assert!(controller.restored.is_none());
}

#[test]
fn waiting_then_ready_preserves_controller_and_phase_identity() {
    let mut controller = ModelController::ready("controller-a");
    controller
        .rechecks
        .push_back(Ok(ControllerTimePendingOwnerStep::Waiting));
    controller
        .rechecks
        .push_back(Ok(ControllerTimePendingOwnerStep::Ready(
            ControllerTimeSample::for_validation(7),
        )));
    let pending = TimedPreparationPending::begin(controller, Phase("admission"), rollback)
        .expect("the request begins");
    let TimedPreparationStep::Waiting(pending) = pending.recheck() else {
        panic!("the first bounded observation must wait");
    };
    let TimedPreparationStep::Ready {
        controller,
        phase,
        sample,
    } = pending.recheck()
    else {
        panic!("the second bounded observation must complete");
    };
    assert_eq!(controller.identity, "controller-a");
    assert_eq!(phase, Phase("admission"));
    assert_eq!(sample, ControllerTimeSample::for_validation(7));
    assert_eq!(controller.restored, None);
}

#[test]
fn failed_recheck_rolls_back_the_exact_phase_before_fail_stop() {
    let mut controller = ModelController::ready("controller-f");
    controller
        .rechecks
        .push_back(Err(ControllerTimeEventError::OwnershipLost));
    let pending = TimedPreparationPending::begin(controller, Phase("sequence"), rollback)
        .expect("the request begins");

    let TimedPreparationStep::FailStop(failure) = pending.recheck() else {
        panic!("a lost controller-time owner must fail closed");
    };
    assert_eq!(
        failure.cause(),
        TimedPreparationFailStopCause::ControllerTime(
            ControllerTimeAcquisitionError::OwnershipLost
        )
    );
    let (controller, rollback_owner) = failure.into_parts();
    assert_eq!(controller.identity, "controller-f");
    assert_eq!(controller.restored, Some(Phase("sequence")));
    assert!(rollback_owner.is_none());
}

#[test]
fn begin_failure_rolls_back_but_seals_the_non_idle_controller() {
    let mut controller = ModelController::ready("controller-b");
    controller.request = Some(Err(ControllerTimeAcquisitionError::Busy));
    let failure = TimedPreparationPending::begin(controller, Phase("sequence"), rollback)
        .expect_err("a busy worker cannot accept another preparation");
    assert_eq!(
        failure.cause(),
        TimedPreparationFailStopCause::ControllerTime(ControllerTimeAcquisitionError::Busy)
    );
    let (controller, rollback_owner) = failure.into_parts();
    assert_eq!(controller.identity, "controller-b");
    assert_eq!(controller.restored, Some(Phase("sequence")));
    assert!(rollback_owner.is_none());
}

#[test]
fn cancellation_waits_for_exact_orphan_drain_before_recovery() {
    let mut controller = ModelController::ready("controller-c");
    controller
        .drains
        .push_back(Ok(ControllerTimePendingOrphanStep::Waiting));
    controller
        .drains
        .push_back(Ok(ControllerTimePendingOrphanStep::Drained));
    let pending = TimedPreparationPending::begin(controller, Phase("timing"), rollback)
        .expect("the request begins");
    let cancellation = pending.cancel().expect("rollback succeeds");
    let TimedPreparationCancellationStep::Waiting(cancellation) =
        cancellation.recheck::<RollbackOwner>()
    else {
        panic!("the hardware request remains abandoned");
    };
    let TimedPreparationCancellationStep::Recovered(controller) =
        cancellation.recheck::<RollbackOwner>()
    else {
        panic!("only the drained observation recovers the controller");
    };
    assert_eq!(controller.identity, "controller-c");
    assert_eq!(controller.restored, Some(Phase("timing")));
}

#[test]
fn rollback_failure_retains_the_exact_unrestored_owner() {
    let mut controller = ModelController::ready("controller-d");
    controller.rollback_failure = Some(RollbackOwner("graph-d"));
    let pending = TimedPreparationPending::begin(controller, Phase("admission"), rollback)
        .expect("the request begins");
    let failure = pending
        .cancel()
        .expect_err("an unrestored role graph is permanently sealed");
    assert_eq!(failure.cause(), TimedPreparationFailStopCause::Rollback);
    let (controller, rollback_owner) = failure.into_parts();
    assert_eq!(controller.identity, "controller-d");
    assert_eq!(rollback_owner, Some(RollbackOwner("graph-d")));
}

#[test]
fn idle_during_cancel_drain_is_phase_ownership_fail_stop() {
    let mut controller = ModelController::ready("controller-e");
    controller
        .drains
        .push_back(Ok(ControllerTimePendingOrphanStep::Idle));
    let pending = TimedPreparationPending::begin(controller, Phase("sequence"), rollback)
        .expect("the request begins");
    let cancellation = pending.cancel().expect("rollback succeeds");
    let TimedPreparationCancellationStep::FailStop(failure) =
        cancellation.recheck::<RollbackOwner>()
    else {
        panic!("Idle cannot substitute for the exact Drained observation");
    };
    assert_eq!(
        failure.cause(),
        TimedPreparationFailStopCause::PhaseOwnership
    );
    assert_eq!(failure.into_parts().0.identity, "controller-e");
}
