use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerTimeLatchBeginError, BluetoothControllerTimeLatchStep,
    BluetoothControllerTimeLatchStepError,
};

use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerHalInitConfig, BluetoothControllerLatchedTime, BluetoothHalInitPeriod,
    BluetoothHalInitScale,
};

use std::{collections::VecDeque, vec::Vec};

use super::{
    ControllerSchedulerEpoch, ControllerSchedulerNow, ControllerTimeEventError,
    ControllerTimeEventStep, ControllerTimeHardware, ControllerTimePendingCore,
    ControllerTimePendingCoreStep, ControllerTimePendingOrphanStep, ControllerTimePendingOwner,
    ControllerTimePendingOwnerStep, ControllerTimeRequest, ControllerTimeRequestError,
    ControllerTimeSample, ControllerTimeWorker, ControllerTimeWorkerPhase,
    drain_controller_time_orphan,
};

#[test]
fn retirement_preserves_live_and_orphan_requests_until_their_actual_completion() {
    use super::ControllerTimeRetirementError as Error;
    let mut worker = ControllerTimeWorker::new_idle();
    let mut hardware = ModelHardware::new([
        BluetoothControllerTimeLatchStep::Waiting,
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(19)),
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(23)),
    ]);
    assert_eq!(worker.retirement_ready(), Ok(()));
    let request = worker.request(&mut hardware).unwrap();
    assert_eq!(worker.retirement_ready(), Err(Error::Requested));
    assert_eq!(hardware.operations, [Operation::Begin]);
    worker.cancel_owned(request).unwrap();
    assert_eq!(worker.retirement_ready(), Err(Error::OrphanPending));
    assert_eq!(hardware.operations, [Operation::Begin]);
    assert_eq!(
        worker.drain_orphan(&mut hardware),
        Ok(ControllerTimeEventStep::Waiting)
    );
    assert_eq!(worker.retirement_ready(), Err(Error::OrphanPending));
    assert_eq!(
        worker.drain_orphan(&mut hardware),
        Ok(ControllerTimeEventStep::OrphanDrained)
    );
    assert_eq!(worker.retirement_ready(), Ok(()));
    let fresh = worker.request(&mut hardware).unwrap();
    assert_ne!(fresh, request);
    assert!(matches!(
        worker.recheck_owned(fresh, &mut hardware),
        Ok(ControllerTimeEventStep::Sample { request: owned, .. }) if owned == fresh
    ));
    assert_eq!(worker.retirement_ready(), Ok(()));
    // A late cancellation must not turn an ownership fault into idle retirement.
    assert_eq!(
        worker.cancel_owned(request),
        Err(ControllerTimeEventError::RequestMismatch)
    );
    assert_eq!(worker.retirement_ready(), Err(Error::Faulted));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Begin,
    Step,
}

struct ModelHardware {
    in_flight: bool,
    operations: Vec<Operation>,
    steps: VecDeque<BluetoothControllerTimeLatchStep>,
}

impl ModelHardware {
    fn new(steps: impl IntoIterator<Item = BluetoothControllerTimeLatchStep>) -> Self {
        Self {
            in_flight: false,
            operations: Vec::new(),
            steps: steps.into_iter().collect(),
        }
    }
}

impl ControllerTimeHardware for ModelHardware {
    fn begin_controller_time_latch(
        &mut self,
    ) -> Result<(), BluetoothControllerTimeLatchBeginError> {
        if self.in_flight {
            return Err(BluetoothControllerTimeLatchBeginError::AlreadyInFlight);
        }
        self.in_flight = true;
        self.operations.push(Operation::Begin);
        Ok(())
    }

    fn step_controller_time_latch(
        &mut self,
    ) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError> {
        if !self.in_flight {
            return Err(BluetoothControllerTimeLatchStepError::NotInFlight);
        }
        let step = self
            .steps
            .pop_front()
            .expect("test hardware needs one scripted event step");
        self.operations.push(Operation::Step);
        if matches!(step, BluetoothControllerTimeLatchStep::Ready(_)) {
            self.in_flight = false;
        }
        Ok(step)
    }
}

#[derive(Debug)]
struct PendingOwnerModel {
    identity: u8,
    rechecks: Vec<ControllerTimeRequest>,
    cancellations: Vec<ControllerTimeRequest>,
    recheck_steps: VecDeque<Result<ControllerTimePendingOwnerStep, ControllerTimeEventError>>,
    cancel_error: Option<ControllerTimeEventError>,
    orphan_steps: VecDeque<Result<ControllerTimePendingOrphanStep, ControllerTimeEventError>>,
}

impl PendingOwnerModel {
    fn new(identity: u8) -> Self {
        Self {
            identity,
            rechecks: Vec::new(),
            cancellations: Vec::new(),
            recheck_steps: VecDeque::new(),
            cancel_error: None,
            orphan_steps: VecDeque::new(),
        }
    }
}

impl ControllerTimePendingOwner for PendingOwnerModel {
    fn recheck_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<ControllerTimePendingOwnerStep, ControllerTimeEventError> {
        self.rechecks.push(request);
        self.recheck_steps
            .pop_front()
            .expect("pending-owner model needs one recheck step")
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<(), ControllerTimeEventError> {
        self.cancellations.push(request);
        match self.cancel_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<ControllerTimePendingOrphanStep, ControllerTimeEventError> {
        self.orphan_steps
            .pop_front()
            .expect("pending-owner model needs one orphan-drain step")
    }
}

const fn sample(raw_time: u32) -> ControllerTimeSample {
    ControllerTimeSample::for_validation(raw_time)
}

#[test]
fn pending_core_drop_cancels_exact_request_then_uses_orphan_drain() {
    let request = ControllerTimeRequest::for_validation(41);
    let mut owner = PendingOwnerModel::new(1);
    owner.orphan_steps.extend([
        Ok(ControllerTimePendingOrphanStep::Waiting),
        Ok(ControllerTimePendingOrphanStep::Drained),
        Ok(ControllerTimePendingOrphanStep::Idle),
    ]);

    drop(ControllerTimePendingCore::new(&mut owner, request));
    assert_eq!(owner.cancellations, [request]);
    assert_eq!(
        drain_controller_time_orphan(&mut owner),
        Ok(ControllerTimePendingOrphanStep::Waiting)
    );
    assert_eq!(
        drain_controller_time_orphan(&mut owner),
        Ok(ControllerTimePendingOrphanStep::Drained)
    );
    assert_eq!(
        drain_controller_time_orphan(&mut owner),
        Ok(ControllerTimePendingOrphanStep::Idle)
    );
}

#[test]
fn pending_core_waiting_reconstructs_the_same_owner_and_request() {
    let request = ControllerTimeRequest::for_validation(42);
    let mut owner = PendingOwnerModel::new(7);
    owner.recheck_steps.extend([
        Ok(ControllerTimePendingOwnerStep::Waiting),
        Ok(ControllerTimePendingOwnerStep::Ready(sample(0x1234))),
    ]);

    let pending = ControllerTimePendingCore::new(&mut owner, request);
    let waiting = match pending.recheck().expect("first recheck waits") {
        ControllerTimePendingCoreStep::Waiting(waiting) => waiting,
        ControllerTimePendingCoreStep::Ready { .. } => {
            panic!("first recheck unexpectedly completed")
        }
    };
    let (identity, observed) = match waiting.recheck().expect("second recheck completes") {
        ControllerTimePendingCoreStep::Ready { owner, sample } => {
            (owner.identity, sample.raw_ticks())
        }
        ControllerTimePendingCoreStep::Waiting(_) => {
            panic!("second recheck unexpectedly waited")
        }
    };

    assert_eq!(identity, 7);
    assert_eq!(observed, 0x1234);
    assert_eq!(owner.rechecks, [request, request]);
    assert!(owner.cancellations.is_empty());
}

#[test]
fn pending_core_ready_disarms_drop_cancellation() {
    let request = ControllerTimeRequest::for_validation(43);
    let mut owner = PendingOwnerModel::new(2);
    owner
        .recheck_steps
        .push_back(Ok(ControllerTimePendingOwnerStep::Ready(sample(9))));

    {
        let ready = ControllerTimePendingCore::new(&mut owner, request)
            .recheck()
            .expect("recheck completes");
        match ready {
            ControllerTimePendingCoreStep::Ready {
                owner: returned,
                sample,
            } => {
                assert_eq!(sample.raw_ticks(), 9);
                let _ = returned;
            }
            ControllerTimePendingCoreStep::Waiting(_) => {
                panic!("recheck unexpectedly waited")
            }
        }
    }

    assert_eq!(owner.rechecks, [request]);
    assert!(owner.cancellations.is_empty());
}

#[test]
fn pending_core_recheck_failure_returns_exact_owner_without_cancellation() {
    let request = ControllerTimeRequest::for_validation(44);
    let mut owner = PendingOwnerModel::new(9);
    owner
        .recheck_steps
        .push_back(Err(ControllerTimeEventError::OwnershipLost));

    let failure = ControllerTimePendingCore::new(owner, request)
        .recheck()
        .expect_err("model rejects the exact recheck");
    let (returned, error) = failure.into_parts();

    assert_eq!(returned.identity, 9);
    assert_eq!(error, ControllerTimeEventError::OwnershipLost);
    assert_eq!(returned.rechecks, [request]);
    assert!(returned.cancellations.is_empty());
}

#[test]
fn pending_core_explicit_cancel_exposes_mismatch_and_fault() {
    for (generation, expected) in [
        (44, ControllerTimeEventError::RequestMismatch),
        (45, ControllerTimeEventError::Faulted),
    ] {
        let request = ControllerTimeRequest::for_validation(generation);
        let mut owner = PendingOwnerModel::new(3);
        owner.cancel_error = Some(expected);

        let failure = ControllerTimePendingCore::new(&mut owner, request)
            .cancel()
            .expect_err("model rejects explicit cancellation");
        let (returned, error) = failure.into_parts();
        assert_eq!(error, expected);
        assert_eq!(returned.identity, 3);
        assert_eq!(owner.cancellations, [request]);
    }
}

#[test]
fn request_and_each_recheck_are_separate_bounded_hardware_steps() {
    let mut hardware = ModelHardware::new([
        BluetoothControllerTimeLatchStep::Waiting,
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(
            0x1234_5678,
        )),
    ]);
    let mut worker = ControllerTimeWorker::new_idle();

    assert_eq!(worker.phase(), ControllerTimeWorkerPhase::Idle);
    let request = worker.request(&mut hardware).expect("request is published");
    assert_eq!(hardware.operations, [Operation::Begin]);
    assert!(worker.needs_recheck());

    assert_eq!(
        worker.recheck_owned(request, &mut hardware),
        Ok(ControllerTimeEventStep::Waiting)
    );
    assert_eq!(hardware.operations, [Operation::Begin, Operation::Step]);
    assert_eq!(worker.phase(), ControllerTimeWorkerPhase::Requested);

    let sample = match worker
        .recheck_owned(request, &mut hardware)
        .expect("second event completes")
    {
        ControllerTimeEventStep::Sample {
            request: completed,
            sample,
        } => {
            assert_eq!(completed, request);
            sample
        }
        other => panic!("unexpected event step: {other:?}"),
    };
    assert_eq!(sample.raw_ticks(), 0x1234_5678);
    assert_eq!(
        hardware.operations,
        [Operation::Begin, Operation::Step, Operation::Step]
    );
    assert_eq!(worker.phase(), ControllerTimeWorkerPhase::Idle);
    assert!(!worker.needs_recheck());
}

#[test]
fn abandoned_request_is_drained_without_becoming_a_new_sample() {
    let mut hardware = ModelHardware::new([
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(
            0x1111_1111,
        )),
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(
            0x2222_2222,
        )),
    ]);
    let mut worker = ControllerTimeWorker::new_idle();

    let abandoned = worker.request(&mut hardware).expect("request is published");
    assert_eq!(worker.cancel_owned(abandoned), Ok(()));
    assert_eq!(worker.phase(), ControllerTimeWorkerPhase::DrainingOrphan);
    assert_eq!(
        worker.drain_orphan(&mut hardware),
        Ok(ControllerTimeEventStep::OrphanDrained)
    );
    assert_eq!(worker.phase(), ControllerTimeWorkerPhase::Idle);

    let fresh = worker
        .request(&mut hardware)
        .expect("fresh request is published");
    let sample = match worker
        .recheck_owned(fresh, &mut hardware)
        .expect("fresh request completes")
    {
        ControllerTimeEventStep::Sample { request, sample } => {
            assert_eq!(request, fresh);
            sample
        }
        other => panic!("unexpected event step: {other:?}"),
    };
    assert_eq!(sample.raw_ticks(), 0x2222_2222);
    assert_eq!(
        hardware.operations,
        [
            Operation::Begin,
            Operation::Step,
            Operation::Begin,
            Operation::Step
        ]
    );
}

#[test]
fn orphan_waiting_blocks_reuse_until_the_abandoned_request_is_drained() {
    let mut hardware = ModelHardware::new([
        BluetoothControllerTimeLatchStep::Waiting,
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(
            0x1111_1111,
        )),
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(
            0x2222_2222,
        )),
    ]);
    let mut worker = ControllerTimeWorker::new_idle();

    let abandoned = worker.request(&mut hardware).expect("request is published");
    worker
        .cancel_owned(abandoned)
        .expect("the exact request can be abandoned");
    assert_eq!(
        worker.drain_orphan(&mut hardware),
        Ok(ControllerTimeEventStep::Waiting)
    );
    assert!(worker.needs_recheck());
    assert!(!worker.is_reunitable());

    let operations_before_retry = hardware.operations.clone();
    assert_eq!(
        worker.request(&mut hardware),
        Err(ControllerTimeRequestError::Busy)
    );
    assert_eq!(hardware.operations, operations_before_retry);

    assert_eq!(
        worker.drain_orphan(&mut hardware),
        Ok(ControllerTimeEventStep::OrphanDrained)
    );
    assert!(worker.is_reunitable());
    assert!(!worker.needs_recheck());

    let fresh = worker
        .request(&mut hardware)
        .expect("the drained worker accepts a fresh request");
    assert!(matches!(
        worker.recheck_owned(fresh, &mut hardware),
        Ok(ControllerTimeEventStep::Sample { request, sample })
            if request == fresh && sample.raw_ticks() == 0x2222_2222
    ));
}

#[test]
fn mismatched_cancellation_faults_without_touching_newer_hardware() {
    let mut hardware = ModelHardware::new([
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(1)),
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(2)),
    ]);
    let mut worker = ControllerTimeWorker::new_idle();

    let first = worker.request(&mut hardware).expect("first request");
    assert!(matches!(
        worker.recheck_owned(first, &mut hardware),
        Ok(ControllerTimeEventStep::Sample {
            request,
            sample: _
        }) if request == first
    ));

    let second = worker.request(&mut hardware).expect("second request");
    assert_ne!(first, second);
    assert_eq!(
        worker.cancel_owned(first),
        Err(ControllerTimeEventError::RequestMismatch)
    );
    assert_eq!(worker.phase(), ControllerTimeWorkerPhase::Faulted);
    assert_eq!(
        worker.cancel_owned(second),
        Err(ControllerTimeEventError::Faulted)
    );
    assert_eq!(
        worker.recheck_owned(second, &mut hardware),
        Err(ControllerTimeEventError::Faulted)
    );
    assert_eq!(
        hardware.operations,
        [Operation::Begin, Operation::Step, Operation::Begin]
    );
}

#[test]
fn mismatched_recheck_faults_before_hardware_observation() {
    let mut hardware = ModelHardware::new([
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(1)),
        BluetoothControllerTimeLatchStep::Ready(BluetoothControllerLatchedTime::from_bits(2)),
    ]);
    let mut worker = ControllerTimeWorker::new_idle();

    let first = worker.request(&mut hardware).expect("first request");
    assert!(matches!(
        worker.recheck_owned(first, &mut hardware),
        Ok(ControllerTimeEventStep::Sample { request, .. }) if request == first
    ));
    let second = worker.request(&mut hardware).expect("second request");

    assert_eq!(
        worker.recheck_owned(first, &mut hardware),
        Err(ControllerTimeEventError::RequestMismatch)
    );
    assert_eq!(worker.phase(), ControllerTimeWorkerPhase::Faulted);
    assert_eq!(
        worker.recheck_owned(second, &mut hardware),
        Err(ControllerTimeEventError::Faulted)
    );
    assert_eq!(
        hardware.operations,
        [Operation::Begin, Operation::Step, Operation::Begin]
    );
}

#[test]
fn lower_ownership_collision_is_sticky_fail_stop() {
    let mut hardware = ModelHardware::new([BluetoothControllerTimeLatchStep::Waiting]);
    let mut worker = ControllerTimeWorker::new_idle();
    hardware.in_flight = true;

    assert_eq!(
        worker.request(&mut hardware),
        Err(ControllerTimeRequestError::OwnershipCollision)
    );
    assert!(hardware.operations.is_empty());
    assert_eq!(worker.phase(), ControllerTimeWorkerPhase::Faulted);
    assert!(!worker.needs_recheck());
    assert_eq!(
        worker.request(&mut hardware),
        Err(ControllerTimeRequestError::Faulted)
    );
}

#[test]
fn idle_orphan_drain_and_duplicate_request_do_not_touch_hardware() {
    let mut hardware = ModelHardware::new([BluetoothControllerTimeLatchStep::Waiting]);
    let mut worker = ControllerTimeWorker::new_idle();

    assert_eq!(
        worker.drain_orphan(&mut hardware),
        Ok(ControllerTimeEventStep::Idle)
    );
    assert!(hardware.operations.is_empty());

    let _request = worker.request(&mut hardware).expect("request is published");
    assert_eq!(
        worker.request(&mut hardware),
        Err(ControllerTimeRequestError::Busy)
    );
    assert_eq!(hardware.operations, [Operation::Begin]);
}

#[test]
fn ownership_loss_is_sticky_fail_stop_without_more_hardware_access() {
    let mut hardware = ModelHardware::new([]);
    let mut worker = ControllerTimeWorker::new_idle();

    let request = worker.request(&mut hardware).expect("request is published");
    hardware.in_flight = false;
    assert_eq!(
        worker.recheck_owned(request, &mut hardware),
        Err(ControllerTimeEventError::OwnershipLost)
    );
    assert_eq!(worker.phase(), ControllerTimeWorkerPhase::Faulted);
    assert_eq!(hardware.operations, [Operation::Begin]);

    assert_eq!(
        worker.recheck_owned(request, &mut hardware),
        Err(ControllerTimeEventError::Faulted)
    );
    assert_eq!(
        worker.request(&mut hardware),
        Err(ControllerTimeRequestError::Faulted)
    );
    assert_eq!(hardware.operations, [Operation::Begin]);
}

#[test]
fn exhausted_generation_faults_before_hardware_publication() {
    let mut hardware = ModelHardware::new([]);
    let mut worker = ControllerTimeWorker::new_idle();
    worker.exhaust_generation_for_validation();

    assert_eq!(
        worker.request(&mut hardware),
        Err(ControllerTimeRequestError::GenerationExhausted)
    );
    assert!(hardware.operations.is_empty());
    assert_eq!(worker.phase(), ControllerTimeWorkerPhase::Faulted);
    assert!(!worker.is_reunitable());
}

#[test]
fn scheduler_epoch_matches_forward_backward_and_wrapping_branches() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = ControllerSchedulerEpoch::new(sample(100), 1_000, scale);

    assert_eq!(epoch.project(sample(103)), 1_001);
    assert_eq!(epoch.project(sample(97)), 998);

    let wrapping_epoch = ControllerSchedulerEpoch::new(sample(0xffff_fffe), 1_000, scale);
    assert_eq!(wrapping_epoch.project(sample(1)), 1_001);

    let reverse_wrapping_epoch = ControllerSchedulerEpoch::new(sample(1), 1_000, scale);
    assert_eq!(reverse_wrapping_epoch.project(sample(0xffff_fffe)), 998);

    assert_eq!(epoch.raw_ticks_for_micros(1_001), 102);
    assert_eq!(epoch.raw_ticks_for_micros(1_015), 130);
    assert_eq!(epoch.raw_ticks_for_micros(998), 96);
    assert_eq!(epoch.raw_ticks_for_micros(985), 70);

    let scheduler_wrapping_epoch = ControllerSchedulerEpoch::new(sample(100), 0xffff_fffe, scale);
    assert_eq!(scheduler_wrapping_epoch.raw_ticks_for_micros(10), 124);
}

#[test]
fn first_live_epoch_uses_each_reviewed_forward_scale() {
    let cases = [
        (
            BluetoothHalInitScale::Eight,
            BluetoothHalInitPeriod::Image2000,
        ),
        (
            BluetoothHalInitScale::Eight,
            BluetoothHalInitPeriod::Image1000,
        ),
        (
            BluetoothHalInitScale::Eight,
            BluetoothHalInitPeriod::Image500,
        ),
        (
            BluetoothHalInitScale::Sixteen,
            BluetoothHalInitPeriod::Image2000,
        ),
        (
            BluetoothHalInitScale::Sixteen,
            BluetoothHalInitPeriod::Image1000,
        ),
        (
            BluetoothHalInitScale::Sixteen,
            BluetoothHalInitPeriod::Image500,
        ),
    ];
    let raw_tick_anchor = 0x1234_5678;

    for (hal_scale, period) in cases {
        let scale = BluetoothControllerHalInitConfig::new(hal_scale, 11, 33, period)
            .controller_time_scale();
        let first_sample = sample(raw_tick_anchor);
        let epoch = ControllerSchedulerEpoch::from_first_live_update(&first_sample, scale);
        let expected = scale.micros_from_raw_ticks(raw_tick_anchor);

        assert_eq!(epoch.project(sample(raw_tick_anchor)), expected);
        assert_eq!(
            epoch.project(sample(raw_tick_anchor.wrapping_add(3))),
            expected.wrapping_add(scale.micros_from_raw_ticks(3))
        );
    }
}

#[test]
fn first_live_epoch_and_affine_now_retain_projected_sample() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let first_sample = sample(0x4000_0000);
    let epoch = ControllerSchedulerEpoch::from_first_live_update(&first_sample, scale);
    let now = ControllerSchedulerNow::from_retained_epoch(epoch, first_sample);

    assert_eq!(now.epoch(), epoch);
    assert_eq!(now.sample().raw_ticks(), 0x4000_0000);
    assert_eq!(now.micros(), 0x2000_0000);
}

#[test]
fn successive_half_microsecond_updates_preserve_time_across_raw_wrap() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    for initial_raw in [101_u32, 0xffff_fffd] {
        let mut epoch =
            ControllerSchedulerEpoch::from_first_live_update(&sample(initial_raw), scale);
        let initial_micros = initial_raw / 2;
        assert_eq!(epoch.raw_ticks_for_micros(initial_micros), initial_raw - 1);
        for elapsed_ticks in 0..16_u32 {
            let raw = initial_raw.wrapping_add(elapsed_ticks);
            let expected = initial_micros.wrapping_add(elapsed_ticks.div_ceil(2));
            epoch = epoch.reanchor(&sample(raw));
            assert_eq!(epoch.project(sample(raw)), expected);
            // A fractional sample keeps the inverse anchored at a whole microsecond.
            assert_eq!(epoch.raw_ticks_for_micros(expected), raw - raw % 2);
            assert_eq!(
                epoch.project(sample(raw.wrapping_sub(1))),
                initial_micros.wrapping_add(elapsed_ticks / 2)
            );
        }
    }
}

#[test]
fn earlier_fractional_microseconds_round_down_without_reanchor_drift() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = ControllerSchedulerEpoch::new(sample(100), 1_000, scale);
    let earlier = epoch.reanchor(&sample(97));
    assert_eq!(earlier.project(sample(97)), 998);
    assert_eq!(earlier.raw_ticks_for_micros(998), 96);
    assert_eq!(earlier.project(sample(100)), 1_000);
}

#[test]
fn slow_tick_inverse_truncates_on_both_sides_of_epoch() {
    let scale = BluetoothControllerHalInitConfig::new(
        BluetoothHalInitScale::Sixteen,
        11,
        33,
        BluetoothHalInitPeriod::Image500,
    )
    .controller_time_scale();
    let epoch = ControllerSchedulerEpoch::new(sample(100), 1_000, scale);
    assert_eq!(epoch.raw_ticks_for_micros(1_003), 101);
    assert_eq!(epoch.raw_ticks_for_micros(997), 99);
}

#[test]
fn post_enable_projection_retains_the_existing_scheduler_epoch() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = ControllerSchedulerEpoch::new(sample(100), 1_000, scale);
    let post_enable_sample = sample(103);

    assert_eq!(epoch.project_without_reanchor(&post_enable_sample), 1_001);
    assert_eq!(epoch.raw_ticks_for_micros(1_001), 102);
}

#[test]
fn physical_duration_admission_enforces_the_retained_scales_half_range() {
    use super::ControllerDurationError;
    use oer_time::RadioDuration;
    for (period, minimum, maximum, ticks) in [
        (
            BluetoothHalInitPeriod::Image500,
            2,
            u64::from(i32::MAX as u32) * 2 + 1,
            i32::MAX as u32,
        ),
        (
            BluetoothHalInitPeriod::Image1000,
            1,
            u64::from(i32::MAX as u32),
            i32::MAX as u32,
        ),
        (
            BluetoothHalInitPeriod::Image2000,
            1,
            u64::from(i32::MAX as u32) / 2,
            i32::MAX as u32 - 1,
        ),
    ] {
        let scale =
            BluetoothControllerHalInitConfig::new(BluetoothHalInitScale::Eight, 11, 33, period)
                .controller_time_scale();
        let epoch =
            ControllerSchedulerEpoch::new(ControllerTimeSample::for_validation(123), 456, scale);
        assert_eq!(
            epoch.raw_duration_ticks(RadioDuration::from_micros(minimum - 1)),
            Err(ControllerDurationError::Empty)
        );
        assert!(
            epoch
                .raw_duration_ticks(RadioDuration::from_micros(minimum))
                .is_ok()
        );
        assert_eq!(
            epoch.raw_duration_ticks(RadioDuration::from_micros(maximum)),
            Ok(ticks)
        );
        assert_eq!(
            epoch.raw_duration_ticks(RadioDuration::from_micros(maximum + 1)),
            Err(ControllerDurationError::BeyondHalfRange)
        );
        assert_eq!(
            epoch.raw_duration_ticks(RadioDuration::from_micros(u64::from(u32::MAX) + 1)),
            Err(ControllerDurationError::BeyondHalfRange)
        );
        assert_eq!(
            epoch.raw_duration_ticks(RadioDuration::from_micros(u64::MAX)),
            Err(ControllerDurationError::BeyondHalfRange)
        );
    }
}
