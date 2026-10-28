use core::{
    future::pending,
    task::{Context, Poll, Waker},
};

use embassy_futures::block_on;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::*;

#[test]
fn retired_epoch_does_not_leave_a_stop_for_its_successor() {
    for consume_stop in [false, true] {
        for repeat_stop in [false, true] {
            let resources = ConnectedTaskControlResources::<NoopRawMutex, u32>::new();
            let (mut old, first) = resources.split().unwrap();
            old.request_stop();
            if consume_stop {
                block_on(first.wait_for_stop());
            }
            first.complete(11);
            block_on(old.wait_stopped());
            if repeat_stop {
                block_on(stop_connected_task_group(&mut old));
            }
            assert_eq!(old.try_finish().ok().unwrap(), 11);

            let (mut current, second) = resources.split().unwrap();
            assert!(poll_once(second.wait_for_stop()).is_pending());
            current.request_stop();
            assert!(poll_once(second.wait_for_stop()).is_ready());
            second.complete(22);
            block_on(current.wait_stopped());
            assert_eq!(current.try_finish().ok().unwrap(), 22);
        }
    }
}

#[test]
fn completion_without_a_stop_and_repeated_observation_retire_cleanly() {
    let resources = ConnectedTaskControlResources::<NoopRawMutex, u32>::new();
    let (mut old, first) = resources.split().unwrap();
    first.complete(11);
    block_on(old.wait_stopped());
    for _ in 0..2 {
        block_on(stop_connected_task_group(&mut old));
    }
    assert_eq!(old.try_finish().ok().unwrap(), 11);
    let (mut next, second) = resources.split().unwrap();
    assert!(poll_once(second.wait_for_stop()).is_pending());
    second.complete(22);
    block_on(next.wait_stopped());
    assert_eq!(next.try_finish().ok().unwrap(), 22);
}

struct ReadyTaskGroup {
    stop_requests: usize,
    owner: Option<u32>,
}

#[test]
fn auxiliary_group_reveals_primary_owner_only_after_both_tasks_stop() {
    let primary = ReadyTaskGroup {
        stop_requests: 0,
        owner: Some(17),
    };
    let auxiliary = ReadyTaskGroup {
        stop_requests: 0,
        owner: Some(29),
    };
    let mut group = ConnectedTaskGroupWithAuxiliary::new(primary, auxiliary);

    group.request_stop();
    block_on(group.wait_stopped());
    let TaskCompletion::Waiting(primary) = &group.primary else {
        panic!()
    };
    let TaskCompletion::Waiting(auxiliary) = &group.auxiliary else {
        panic!()
    };
    assert_eq!(primary.stop_requests, 1);
    assert_eq!(auxiliary.stop_requests, 1);
    assert_eq!(group.try_finish().ok().unwrap(), 17);
}

impl ConnectedTaskGroup for ReadyTaskGroup {
    type Stopped = u32;

    fn request_stop(&mut self) {
        self.stop_requests += 1;
    }

    async fn wait_stopped(&mut self) {}

    fn try_finish(mut self) -> Result<Self::Stopped, Self> {
        match self.owner.take() {
            Some(owner) => Ok(owner),
            None => Err(self),
        }
    }
}

#[test]
fn one_deadline_returns_the_exact_task_owner() {
    let mut group = ReadyTaskGroup {
        stop_requests: 0,
        owner: Some(73),
    };
    let outcome = block_on(stop_connected_task_group_until(
        &mut group,
        &oer_time_virtual::SkipClock::new(),
        Duration::from_millis(1),
    ));
    let ConnectedTaskStopAttempt::Stopped = outcome else {
        panic!("ready task group unexpectedly remained pending");
    };
    assert_eq!(group.stop_requests, 1);
    assert_eq!(group.try_finish().ok().unwrap(), 73);
}

#[test]
fn normal_stop_waits_for_and_returns_the_exact_task_owner() {
    let mut group = ReadyTaskGroup {
        stop_requests: 0,
        owner: Some(79),
    };

    block_on(stop_connected_task_group(&mut group));
    assert_eq!(group.stop_requests, 1);
    assert_eq!(group.try_finish().ok().unwrap(), 79);
}

struct StuckTaskGroup {
    stop_requested: bool,
}

impl ConnectedTaskGroup for StuckTaskGroup {
    type Stopped = ();

    fn request_stop(&mut self) {
        self.stop_requested = true;
    }

    async fn wait_stopped(&mut self) {
        pending().await
    }

    fn try_finish(self) -> Result<Self::Stopped, Self> {
        Err(self)
    }
}

#[test]
fn missed_group_deadline_remains_a_pending_stop() {
    let mut group = StuckTaskGroup {
        stop_requested: false,
    };
    let outcome = block_on(stop_connected_task_group_until(
        &mut group,
        &oer_time_virtual::SkipClock::new(),
        Duration::ZERO,
    ));
    assert!(group.stop_requested);
    assert!(outcome.is_pending());
}

#[test]
fn split_capabilities_round_trip_and_allow_a_later_epoch() {
    let resources = ConnectedTaskControlResources::<NoopRawMutex, u32>::new();
    let (mut controller, endpoint) = resources.split().expect("first epoch is idle");
    controller.request_stop();
    block_on(endpoint.wait_for_stop());
    endpoint.complete(91);
    block_on(controller.wait_stopped());
    assert!(matches!(
        resources.split(),
        Err(ConnectedTaskControlError::Active)
    ));
    assert_eq!(controller.try_finish().ok().unwrap(), 91);
    assert!(resources.split().is_ok());
}

#[test]
fn unused_reservation_rolls_back_without_poisoning_the_next_epoch() {
    let resources = ConnectedTaskControlResources::<NoopRawMutex, u32>::new();
    resources
        .reserve()
        .expect("first epoch is idle")
        .abort_unused();

    assert!(resources.reserve().is_ok());
}

#[test]
fn dropped_reservation_preserves_the_fail_closed_contract() {
    let resources = ConnectedTaskControlResources::<NoopRawMutex, u32>::new();
    drop(resources.reserve().expect("first epoch is idle"));

    assert!(matches!(
        resources.reserve(),
        Err(ConnectedTaskControlError::Poisoned)
    ));
}

#[test]
fn dropped_task_endpoint_permanently_poisons_reuse() {
    let resources = ConnectedTaskControlResources::<NoopRawMutex, u32>::new();
    let (_controller, endpoint) = resources.split().expect("first epoch is idle");
    drop(endpoint);
    assert!(resources.is_poisoned());
    assert!(matches!(
        resources.split(),
        Err(ConnectedTaskControlError::Poisoned)
    ));
}

#[test]
fn dropped_parent_controller_permanently_poisons_reuse() {
    let resources = ConnectedTaskControlResources::<NoopRawMutex, u32>::new();
    let (controller, endpoint) = resources.split().expect("first epoch is idle");
    drop(controller);
    assert!(resources.is_poisoned());
    drop(endpoint);
    assert!(matches!(
        resources.split(),
        Err(ConnectedTaskControlError::Poisoned)
    ));
}

fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
    core::pin::pin!(future).poll(&mut Context::from_waker(Waker::noop()))
}

struct Tracked(std::rc::Rc<core::cell::Cell<usize>>);
impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn cancelled_and_expired_group_observations_retain_both_owners_in_either_completion_order() {
    for primary_first in [false, true] {
        for early_finish in [false, true] {
            let primary_dropped = std::rc::Rc::new(core::cell::Cell::new(0));
            let auxiliary_dropped = std::rc::Rc::new(core::cell::Cell::new(0));
            let primary = ConnectedTaskControlResources::<NoopRawMutex, Tracked>::new();
            let auxiliary = ConnectedTaskControlResources::<NoopRawMutex, Tracked>::new();
            let (primary_control, primary_task) = primary.split().unwrap();
            let (auxiliary_control, auxiliary_task) = auxiliary.split().unwrap();
            let mut group =
                ConnectedTaskGroupWithAuxiliary::new(primary_control, auxiliary_control);
            assert!(poll_once(stop_connected_task_group(&mut group)).is_pending());
            let pending = if primary_first {
                primary_task.complete(Tracked(primary_dropped.clone()));
                (auxiliary_task, auxiliary_dropped.clone())
            } else {
                auxiliary_task.complete(Tracked(auxiliary_dropped.clone()));
                (primary_task, primary_dropped.clone())
            };
            assert!(poll_once(group.wait_stopped()).is_pending());
            assert!(
                block_on(stop_connected_task_group_until(
                    &mut group,
                    &oer_time_virtual::SkipClock::new(),
                    Duration::ZERO,
                ))
                .is_pending()
            );
            assert_eq!(primary_dropped.get(), 0);
            assert_eq!(auxiliary_dropped.get(), 0);
            if early_finish {
                group = group
                    .try_finish()
                    .err()
                    .expect("one child still owns its task");
                assert_eq!(primary_dropped.get(), 0);
                assert_eq!(auxiliary_dropped.get(), 0);
            }
            pending.0.complete(Tracked(pending.1));
            // Even a fully ready observation returns no resources. Cancelling
            // or repeating it leaves both owners inside this same group.
            assert!(poll_once(group.wait_stopped()).is_ready());
            assert!(poll_once(group.wait_stopped()).is_ready());
            assert_eq!(primary_dropped.get(), 0);
            assert_eq!(auxiliary_dropped.get(), 0);
            let stopped = group.try_finish().ok().expect("both children stopped");
            assert_eq!(primary_dropped.get(), 0);
            assert_eq!(auxiliary_dropped.get(), 1);
            assert!(!primary.is_poisoned());
            assert!(!auxiliary.is_poisoned());
            drop(stopped);
            assert_eq!(primary_dropped.get(), 1);
        }
    }
}
