use core::num::NonZeroU32;
use std::vec::Vec;

use oer_esp32s31_bluetooth_memory::{ControllerSramLinkAddress, SchedulerItemCompletionStatus};
use oer_esp32s31_hal::bluetooth::BluetoothSchedulerBusyObservation;

use super::{SchedulerExecutor, SchedulerItemAccess, SchedulerSubmitError};
use crate::scheduler::{list::SchedulerListInsertError, window::SchedulerRawWindow};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Item {
    next: Option<ControllerSramLinkAddress>,
    previous: Option<ControllerSramLinkAddress>,
    status: Option<SchedulerItemCompletionStatus>,
    prepared: bool,
    deleted: bool,
}

/// Item memory for event identities 0..8.
struct Items([Item; 8]);

impl Items {
    fn new() -> Self {
        Self([Item::default(); 8])
    }

    fn execute(&mut self, id: u8, status: u32) {
        self.0[usize::from(id)].status = Some(NonZeroU32::new(status).map_or(
            SchedulerItemCompletionStatus::Zero,
            SchedulerItemCompletionStatus::NonZero,
        ));
    }

    fn chain(&self, head: ControllerSramLinkAddress) -> Vec<u8> {
        let mut order = Vec::new();
        let mut cursor = Some(head);
        while let Some(link) = cursor {
            let id = id_of(link);
            order.push(id);
            cursor = self.0[usize::from(id)].next;
        }
        order
    }
}

fn link(id: u8) -> ControllerSramLinkAddress {
    ControllerSramLinkAddress::new(0x2f00_1000 + 0x100 * u32::from(id))
        .expect("test link is representable")
}

fn id_of(link: ControllerSramLinkAddress) -> u8 {
    (0..8)
        .find(|id| self::link(*id) == link)
        .expect("test link names an item")
}

impl SchedulerItemAccess<u8> for Items {
    fn link(&self, id: u8) -> ControllerSramLinkAddress {
        link(id)
    }

    fn prepare_for_list(
        &mut self,
        id: u8,
        previous: Option<ControllerSramLinkAddress>,
        next: Option<ControllerSramLinkAddress>,
    ) {
        self.0[usize::from(id)] = Item {
            next,
            previous,
            status: None,
            prepared: true,
            deleted: false,
        };
    }

    fn set_next(&mut self, id: u8, next: Option<ControllerSramLinkAddress>) {
        self.0[usize::from(id)].next = next;
    }

    fn set_previous(&mut self, id: u8, previous: Option<ControllerSramLinkAddress>) {
        self.0[usize::from(id)].previous = previous;
    }

    fn completion_status(&self, id: u8) -> Option<SchedulerItemCompletionStatus> {
        self.0[usize::from(id)].status
    }

    fn mark_deleted(&mut self, id: u8) {
        self.0[usize::from(id)].deleted = true;
    }
}

fn idle() -> BluetoothSchedulerBusyObservation {
    BluetoothSchedulerBusyObservation::from_busy_for_validation(false)
}

fn busy() -> BluetoothSchedulerBusyObservation {
    BluetoothSchedulerBusyObservation::from_busy_for_validation(true)
}

fn window(start: u32) -> SchedulerRawWindow {
    SchedulerRawWindow::new(start, start + 50).expect("test window is valid")
}

#[test]
fn idle_insertion_links_the_chain_in_start_order() {
    let mut executor = SchedulerExecutor::<u8, 4>::new();
    let mut items = Items::new();

    let first = executor
        .submit_idle(&mut items, &idle(), 1, window(100))
        .unwrap();
    assert_eq!(first.head, link(1));
    let _ = executor
        .submit_idle(&mut items, &idle(), 2, window(300))
        .unwrap();
    let _ = executor
        .submit_idle(&mut items, &idle(), 3, window(200))
        .unwrap();
    let head = executor
        .submit_idle(&mut items, &idle(), 4, window(0))
        .unwrap();

    assert_eq!(head.head, link(4));
    assert_eq!(items.chain(head.head), [4, 1, 3, 2]);
    assert!(items.0[4].prepared);
    assert_eq!(items.0[4].previous, None);
    assert_eq!(items.0[1].previous, Some(link(4)));
    assert_eq!(items.0[3].previous, Some(link(1)));
    assert_eq!(items.0[2].previous, Some(link(3)));
    assert_eq!(items.0[2].next, None);
}

#[test]
fn rejected_submissions_change_no_item() {
    let mut executor = SchedulerExecutor::<u8, 4>::new();
    let mut items = Items::new();
    let _ = executor
        .submit_idle(&mut items, &idle(), 1, window(100))
        .unwrap();
    let before = items.0;

    assert_eq!(
        executor.submit_idle(&mut items, &busy(), 2, window(300)),
        Err(SchedulerSubmitError::SchedulerBusy)
    );
    assert_eq!(
        executor.submit_idle(&mut items, &idle(), 2, window(120)),
        Err(SchedulerSubmitError::List(
            SchedulerListInsertError::Overlap { with: 1 }
        ))
    );
    assert_eq!(items.0, before);
    assert_eq!(executor.list().len(), 1);
}

#[test]
fn completion_takes_executed_events_from_the_head() {
    let mut executor = SchedulerExecutor::<u8, 4>::new();
    let mut items = Items::new();
    for (id, start) in [(1, 0), (2, 100), (3, 200)] {
        let _ = executor
            .submit_idle(&mut items, &idle(), id, window(start))
            .unwrap();
    }

    let none = executor.take_completed(&mut items).unwrap();
    assert!(none.is_empty());

    items.execute(1, 0);
    items.execute(2, 7);
    let completion = executor.take_completed(&mut items).unwrap();
    let completed: Vec<_> = completion.iter().collect();
    assert_eq!(
        completed,
        [
            (1, SchedulerItemCompletionStatus::Zero),
            (
                2,
                SchedulerItemCompletionStatus::NonZero(NonZeroU32::new(7).unwrap())
            ),
        ]
    );
    assert!(!completion.out_of_order());
    assert_eq!(executor.list().head().map(|(id, _)| id), Some(3));
    assert_eq!(items.0[3].previous, None);
}

#[test]
fn out_of_order_completion_leaves_the_later_event_listed() {
    let mut executor = SchedulerExecutor::<u8, 4>::new();
    let mut items = Items::new();
    for (id, start) in [(1, 0), (2, 100), (3, 200)] {
        let _ = executor
            .submit_idle(&mut items, &idle(), id, window(start))
            .unwrap();
    }
    items.execute(1, 0);
    items.execute(3, 0);

    let completion = executor.take_completed(&mut items).unwrap();
    assert_eq!(completion.len(), 1);
    assert!(completion.out_of_order());
    let listed: Vec<_> = executor.list().iter().map(|(id, _)| id).collect();
    assert_eq!(listed, [2, 3]);
}

mod live {
    use std::vec::Vec;

    use oer_esp32s31_hal::bluetooth::{
        BluetoothSchedulerExecutionLockDisposition as Lock,
        BluetoothSchedulerExecutionModifyDisposition as Modify,
        BluetoothSchedulerLockModifyObservation,
    };

    use super::{Items, busy, idle, link, window};
    use crate::scheduler::executor::{
        SchedulerAction as Action, SchedulerExecutor, SchedulerNext as Next,
        SchedulerObservation as Observation, SchedulerStep, SchedulerSubmitError,
        SchedulerTransactionFault, SchedulerWait as Wait,
    };

    fn actions(step: &SchedulerStep<u8, 4>) -> Vec<Action> {
        step.actions().collect()
    }

    fn lock_modify(busy: bool, start: bool) -> Observation {
        Observation::LockModify(
            BluetoothSchedulerLockModifyObservation::from_fields_for_validation(busy, start, 0),
        )
    }

    fn running_list() -> (SchedulerExecutor<u8, 4>, Items) {
        let mut executor = SchedulerExecutor::new();
        let mut items = Items::new();
        let _ = executor
            .submit_idle(&mut items, &idle(), 1, window(0))
            .unwrap();
        let _ = executor
            .submit_idle(&mut items, &idle(), 2, window(200))
            .unwrap();
        (executor, items)
    }

    #[test]
    fn a_retained_lock_links_behind_the_predecessor_and_releases_after_lock_modify() {
        let (mut executor, mut items) = running_list();

        let step = executor
            .begin_live_insertion(&items, &busy(), 3, window(100))
            .unwrap();
        assert_eq!(actions(&step), [Action::PublishExecutionLock(link(1))]);
        assert_eq!(step.next(), Next::Await(Wait::ExecutionLock));
        // Nothing is linked before the lock is granted.
        assert_eq!(items.chain(link(1)), [1, 2]);

        let step = executor
            .advance(&mut items, Observation::ExecutionLock(Lock::Pending))
            .unwrap();
        assert_eq!(step.next(), Next::Await(Wait::ExecutionLock));

        let step = executor
            .advance(
                &mut items,
                Observation::ExecutionLock(Lock::ExecutionLockRetained),
            )
            .unwrap();
        assert_eq!(actions(&step), [Action::PublishLockModify(link(3))]);
        assert_eq!(items.chain(link(1)), [1, 3, 2]);

        // The mirror is frozen until the insertion ends.
        assert!(executor.take_completed(&mut items).is_err());
        assert_eq!(
            executor.submit_idle(&mut items, &idle(), 4, window(400)),
            Err(SchedulerSubmitError::TransactionActive)
        );

        let step = executor
            .advance(&mut items, lock_modify(true, true))
            .unwrap();
        assert_eq!(step.next(), Next::Await(Wait::LockModify));
        let step = executor
            .advance(&mut items, lock_modify(true, false))
            .unwrap();
        assert_eq!(actions(&step), [Action::ReleaseExecutionLock]);
        assert_eq!(step.next(), Next::Finished);
        assert!(executor.take_completed(&mut items).is_ok());
    }

    #[test]
    fn a_lock_that_is_not_retained_falls_back_to_modify_and_publishes_the_new_head() {
        let (mut executor, mut items) = running_list();
        let _ = executor
            .begin_live_insertion(&items, &busy(), 3, window(100))
            .unwrap();

        let step = executor
            .advance(
                &mut items,
                Observation::ExecutionLock(Lock::ReconcileCurrentHead),
            )
            .unwrap();
        assert_eq!(
            actions(&step),
            [Action::ReleaseExecutionLock, Action::PublishExecutionModify]
        );
        assert_eq!(step.next(), Next::Await(Wait::ExecutionModify));

        let step = executor
            .advance(&mut items, Observation::ExecutionModify(Modify::Ready))
            .unwrap();
        assert_eq!(
            actions(&step),
            [
                Action::PublishHead(Some(link(3))),
                Action::ReleaseExecutionModify
            ]
        );
        assert_eq!(step.next(), Next::Finished);
        assert_eq!(items.chain(link(1)), [1, 3, 2]);
    }

    #[test]
    fn a_new_first_event_uses_modify() {
        let (mut executor, mut items) = running_list();
        let step = executor
            .begin_live_insertion(&items, &busy(), 3, window(0xffff_ff00))
            .unwrap();
        assert_eq!(actions(&step), [Action::PublishExecutionModify]);
        let step = executor
            .advance(&mut items, Observation::ExecutionModify(Modify::Ready))
            .unwrap();
        assert_eq!(
            actions(&step),
            [
                Action::PublishHead(Some(link(3))),
                Action::ReleaseExecutionModify
            ]
        );
        assert_eq!(items.chain(link(3)), [3, 1, 2]);
    }

    #[test]
    fn faults_end_the_insertion_without_linking() {
        let (mut executor, mut items) = running_list();
        let _ = executor
            .begin_live_insertion(&items, &busy(), 3, window(100))
            .unwrap();
        assert_eq!(
            executor.advance(&mut items, Observation::ExecutionModify(Modify::Ready)),
            Err(SchedulerTransactionFault::UnexpectedObservation)
        );
        assert_eq!(
            executor.advance(
                &mut items,
                Observation::ExecutionLock(Lock::UnsupportedHardwareResult)
            ),
            Err(SchedulerTransactionFault::UnsupportedExecutionLockResult)
        );
        assert_eq!(items.chain(link(1)), [1, 2]);
        assert!(!executor.list().contains(3));

        let _ = executor
            .begin_live_insertion(&items, &busy(), 3, window(0xffff_ff00))
            .unwrap();
        assert_eq!(
            executor.advance(
                &mut items,
                Observation::ExecutionModify(Modify::HardwareRejected)
            ),
            Err(SchedulerTransactionFault::ExecutionModifyRejected)
        );
        assert!(!executor.list().contains(3));
        assert_eq!(
            executor.advance(&mut items, Observation::ExecutionModify(Modify::Ready)),
            Err(SchedulerTransactionFault::NoTransaction)
        );
    }

    #[test]
    fn live_insertion_needs_a_running_scheduler_and_restart_follows_an_idle_one() {
        let (mut executor, mut items) = running_list();
        assert_eq!(
            executor.begin_live_insertion(&items, &idle(), 3, window(100)),
            Err(SchedulerSubmitError::SchedulerIdle)
        );
        items.execute(1, 0);
        let restart = executor.restart_if_idle(&items, &idle()).unwrap();
        assert_eq!(restart.head, link(2));
        assert!(executor.restart_if_idle(&items, &busy()).is_none());
    }
}

mod cancel {
    use std::vec::Vec;

    use oer_esp32s31_bluetooth_memory::{ControllerSramLinkAddress, SchedulerItemCompletionStatus};
    use oer_esp32s31_hal::bluetooth::{
        BluetoothSchedulerBusyObservation, BluetoothSchedulerCancellationDisposition as Hold,
        BluetoothSchedulerExecutionModifyDisposition as Modify,
        BluetoothSchedulerLockModifyObservation, BluetoothSchedulerSkipDisposition as Skip,
        BluetoothSchedulerSkipResult as SkipResult, BluetoothSchedulerStopped,
    };

    use super::{Items, busy, idle, link, window};
    use crate::scheduler::executor::{
        SchedulerAction as Action, SchedulerCancelError, SchedulerExecutor, SchedulerNext as Next,
        SchedulerNotStopped, SchedulerObservation as Observation, SchedulerStep,
        SchedulerStopRejected, SchedulerSubmitError, SchedulerTransactionActive,
        SchedulerTransactionFault, SchedulerWait as Wait,
    };

    type Executor = SchedulerExecutor<u8, 4>;
    type Step = SchedulerStep<u8, 4>;

    fn actions(step: &Step) -> Vec<Action> {
        step.actions().collect()
    }

    fn released(step: &Step) -> Vec<(u8, Option<SchedulerItemCompletionStatus>)> {
        step.released().iter().collect()
    }

    fn listed(executor: &Executor) -> Vec<u8> {
        executor.list().iter().map(|(id, _)| id).collect()
    }

    fn lock_modify(start: bool) -> Observation {
        Observation::LockModify(
            BluetoothSchedulerLockModifyObservation::from_fields_for_validation(true, start, 0),
        )
    }

    /// Events 1..=4 at starts 0, 100, 200 and 300.
    fn four_events() -> (Executor, Items) {
        let mut executor = SchedulerExecutor::new();
        let mut items = Items::new();
        for id in 1..=4 {
            let _ = executor
                .submit_idle(&mut items, &idle(), id, window((u32::from(id) - 1) * 100))
                .unwrap();
        }
        (executor, items)
    }

    /// Start a running cancellation and open its hold.
    fn open_hold(
        executor: &mut Executor,
        items: &mut Items,
        scheduler: BluetoothSchedulerBusyObservation,
        head: Option<ControllerSramLinkAddress>,
        ids: &[u8],
    ) -> Step {
        let step = executor.begin_cancel(items, &scheduler, head, ids).unwrap();
        assert_eq!(actions(&step), []);
        assert_eq!(step.next(), Next::Await(Wait::LockModify));
        let step = executor.advance(items, lock_modify(true)).unwrap();
        assert_eq!(step.next(), Next::Await(Wait::LockModify));
        let step = executor.advance(items, lock_modify(false)).unwrap();
        assert_eq!(
            actions(&step),
            [
                Action::IndexCancellation,
                Action::AcknowledgeCancellationSource,
                Action::RequestCancellation
            ]
        );
        assert_eq!(step.next(), Next::Await(Wait::Cancellation));
        let step = executor
            .advance(items, Observation::Cancellation(Hold::Pending))
            .unwrap();
        assert_eq!(step.next(), Next::Await(Wait::Cancellation));
        executor
            .advance(items, Observation::Cancellation(Hold::Settled))
            .unwrap()
    }

    #[test]
    fn idle_cancellation_unlinks_and_republishes_the_head() {
        let (mut executor, mut items) = four_events();

        let step = executor
            .begin_cancel(&mut items, &idle(), None, &[3, 2])
            .unwrap();
        assert_eq!(actions(&step), [Action::PublishHead(Some(link(1)))]);
        assert_eq!(step.next(), Next::Finished);
        assert_eq!(released(&step), [(2, None), (3, None)]);
        assert_eq!(items.chain(link(1)), [1, 4]);
        assert_eq!(items.0[4].previous, Some(link(1)));
        // A deleted item keeps its own next link for hardware that holds it.
        assert!(items.0[2].deleted && items.0[3].deleted);
        assert_eq!(items.0[3].next, Some(link(4)));

        let step = executor
            .begin_cancel(&mut items, &idle(), None, &[1])
            .unwrap();
        assert_eq!(actions(&step), [Action::PublishHead(Some(link(4)))]);
        assert_eq!(items.0[4].previous, None);
        let step = executor
            .begin_cancel(&mut items, &idle(), None, &[4])
            .unwrap();
        assert_eq!(actions(&step), [Action::PublishHead(None)]);
        assert!(executor.list().is_empty());
    }

    #[test]
    fn rejected_cancellations_change_no_item() {
        let (mut executor, mut items) = four_events();
        let before = items.0;
        assert_eq!(
            executor.begin_cancel(&mut items, &idle(), None, &[9]),
            Err(SchedulerCancelError::NotListed(9))
        );
        assert_eq!(
            executor.begin_cancel(&mut items, &idle(), None, &[2, 2]),
            Err(SchedulerCancelError::NotListed(2))
        );
        assert_eq!(
            executor.begin_cancel(&mut items, &busy(), Some(link(7)), &[2]),
            Err(SchedulerCancelError::ForeignHardwareHead)
        );
        assert_eq!(items.0, before);
        assert_eq!(listed(&executor), [1, 2, 3, 4]);

        items.execute(1, 0);
        let _ = executor.take_completed(&mut items).unwrap();
        let _ = executor
            .begin_live_insertion(&items, &busy(), 5, window(400))
            .unwrap();
        assert_eq!(
            executor.begin_cancel(&mut items, &busy(), None, &[2]),
            Err(SchedulerCancelError::TransactionActive)
        );
    }

    #[test]
    fn running_cancellation_skips_only_unexecuted_events_up_to_the_hardware_head() {
        let (mut executor, mut items) = four_events();
        items.execute(1, 0);

        let step = open_hold(&mut executor, &mut items, busy(), Some(link(2)), &[1, 2, 3]);
        // Event 1 executed and event 3 starts after the hardware head.
        assert_eq!(actions(&step), [Action::PublishSkip(link(2))]);
        assert_eq!(step.next(), Next::Await(Wait::Skip));
        assert_eq!(listed(&executor), [4]);
        assert_eq!(
            executor.take_completed(&mut items).err(),
            Some(SchedulerTransactionActive)
        );
        assert_eq!(
            executor.begin_flush(&items, &busy()).err(),
            Some(SchedulerTransactionActive)
        );

        let step = executor
            .advance(&mut items, Observation::Skip(Skip::Pending))
            .unwrap();
        assert_eq!(step.next(), Next::Await(Wait::Skip));
        assert!(step.released().is_empty());
        let step = executor
            .advance(
                &mut items,
                Observation::Skip(Skip::Completed(SkipResult::One)),
            )
            .unwrap();
        assert_eq!(
            actions(&step),
            [Action::ClearSkip, Action::ReleaseCancellation]
        );
        assert_eq!(step.next(), Next::Finished);
        assert_eq!(
            released(&step),
            [
                (1, Some(SchedulerItemCompletionStatus::Zero)),
                (2, None),
                (3, None)
            ]
        );
        assert!(executor.take_completed(&mut items).is_ok());
    }

    #[test]
    fn without_a_hardware_head_every_unexecuted_event_is_skipped_until_a_terminal_result() {
        let (mut executor, mut items) = four_events();

        let step = open_hold(&mut executor, &mut items, busy(), None, &[1, 2, 3]);
        assert_eq!(actions(&step), [Action::PublishSkip(link(1))]);
        // Execution is sampled when the loop reaches each event.
        items.execute(2, 0);
        let step = executor
            .advance(
                &mut items,
                Observation::Skip(Skip::Completed(SkipResult::Three)),
            )
            .unwrap();
        assert_eq!(
            actions(&step),
            [Action::ClearSkip, Action::PublishSkip(link(3))]
        );
        let step = executor
            .advance(&mut items, Observation::Skip(Skip::SchedulerIdle))
            .unwrap();
        assert_eq!(
            actions(&step),
            [Action::ClearSkip, Action::ReleaseCancellation]
        );
        assert_eq!(step.released().len(), 3);

        let (mut executor, mut items) = four_events();
        let _ = open_hold(&mut executor, &mut items, busy(), None, &[1, 2]);
        let step = executor
            .advance(
                &mut items,
                Observation::Skip(Skip::Completed(SkipResult::Two)),
            )
            .unwrap();
        assert_eq!(
            actions(&step),
            [Action::ClearSkip, Action::ReleaseCancellation]
        );
        assert_eq!(step.next(), Next::Finished);
    }

    #[test]
    fn a_hold_without_skip_targets_closes_at_once() {
        let (mut executor, mut items) = four_events();
        let step = open_hold(&mut executor, &mut items, busy(), Some(link(1)), &[3]);
        assert_eq!(actions(&step), [Action::ReleaseCancellation]);
        assert_eq!(released(&step), [(3, None)]);
    }

    #[test]
    fn an_unsupported_skip_result_withholds_the_detached_events() {
        let (mut executor, mut items) = four_events();
        let _ = open_hold(&mut executor, &mut items, busy(), None, &[1]);
        assert_eq!(
            executor.advance(&mut items, Observation::Cancellation(Hold::Settled)),
            Err(SchedulerTransactionFault::UnexpectedObservation)
        );
        assert_eq!(
            executor.advance(
                &mut items,
                Observation::Skip(Skip::UnsupportedHardwareResult)
            ),
            Err(SchedulerTransactionFault::UnsupportedSkipResult)
        );
        assert_eq!(
            executor.advance(&mut items, Observation::Skip(Skip::Pending)),
            Err(SchedulerTransactionFault::NoTransaction)
        );
        assert_eq!(listed(&executor), [2, 3, 4]);
    }

    #[test]
    fn list_deletion_releases_every_event() {
        let (mut executor, items) = four_events();
        let step = executor.begin_flush(&items, &idle()).unwrap();
        assert_eq!(actions(&step), [Action::PublishHead(None)]);
        assert_eq!(step.released().len(), 4);
        assert!(executor.list().is_empty());

        let (mut executor, mut items) = four_events();
        items.execute(1, 0);
        let step = executor.begin_flush(&items, &busy()).unwrap();
        assert_eq!(actions(&step), [Action::PublishExecutionModifyListDeletion]);
        assert_eq!(step.next(), Next::Await(Wait::ExecutionModify));
        let step = executor
            .advance(&mut items, Observation::ExecutionModify(Modify::Pending))
            .unwrap();
        assert_eq!(step.next(), Next::Await(Wait::ExecutionModify));
        assert_eq!(
            executor.advance(&mut items, Observation::Skip(Skip::Pending)),
            Err(SchedulerTransactionFault::UnexpectedObservation)
        );
        assert_eq!(listed(&executor), [1, 2, 3, 4]);
        let step = executor
            .advance(&mut items, Observation::ExecutionModify(Modify::Ready))
            .unwrap();
        assert_eq!(
            actions(&step),
            [Action::PublishHead(None), Action::ReleaseExecutionModify]
        );
        assert_eq!(
            released(&step)[..2],
            [(1, Some(SchedulerItemCompletionStatus::Zero)), (2, None)]
        );
        assert!(executor.list().is_empty());

        let (mut executor, mut items) = four_events();
        let _ = executor.begin_flush(&items, &busy()).unwrap();
        assert_eq!(
            executor.advance(
                &mut items,
                Observation::ExecutionModify(Modify::HardwareRejected)
            ),
            Err(SchedulerTransactionFault::ExecutionModifyRejected)
        );
        assert_eq!(listed(&executor), [1, 2, 3, 4]);
    }

    #[test]
    fn the_stopped_receipt_blocks_every_start_until_resume() {
        let (mut executor, mut items) = four_events();
        executor
            .enter_stopped(BluetoothSchedulerStopped::for_validation())
            .unwrap();
        assert!(executor.stopped().is_some());
        assert!(matches!(
            executor.enter_stopped(BluetoothSchedulerStopped::for_validation()),
            Err(SchedulerStopRejected::AlreadyStopped(_))
        ));
        assert_eq!(
            executor.submit_idle(&mut items, &idle(), 5, window(400)),
            Err(SchedulerSubmitError::Stopped)
        );
        assert!(executor.restart_if_idle(&items, &idle()).is_none());

        // Stale events are cancelled on the idle path before resuming.
        let step = executor
            .begin_cancel(&mut items, &idle(), None, &[1])
            .unwrap();
        assert_eq!(actions(&step), [Action::PublishHead(Some(link(2)))]);
        items.execute(2, 0);

        let restart = executor.resume(&items).unwrap().unwrap();
        assert_eq!(restart.head, link(3));
        assert!(executor.stopped().is_none());
        assert_eq!(executor.resume(&items), Err(SchedulerNotStopped));
    }

    #[test]
    fn the_scheduler_cannot_stop_inside_a_transaction() {
        let (mut executor, items) = four_events();
        let _ = executor.begin_flush(&items, &busy()).unwrap();
        assert!(matches!(
            executor.enter_stopped(BluetoothSchedulerStopped::for_validation()),
            Err(SchedulerStopRejected::TransactionActive(_))
        ));
        assert!(executor.stopped().is_none());
    }
}

mod pools {
    use std::{boxed::Box, vec::Vec};

    use oer_esp32s31_bluetooth_memory::{
        LegacyAdvertisingPool, LegacyAdvertisingPrimaryChannelPlan, LegacyAdvertisingStorage,
        SchedulerAllocationConfig, SchedulerItemId, SchedulerItemSpace, SchedulerPoolModelAddress,
        SchedulerRolePoolStorage,
    };

    use super::idle;
    use crate::scheduler::{executor::SchedulerExecutor, window::SchedulerRawWindow};

    #[test]
    fn the_executor_links_the_items_of_a_role_pool() {
        let storage = Box::leak(Box::new(SchedulerRolePoolStorage::<
            LegacyAdvertisingStorage,
            1,
        >::new()));
        let numbers = SchedulerAllocationConfig::new(1, 0, 0)
            .unwrap()
            .advertising(0, 1)
            .unwrap();
        let mut pool = LegacyAdvertisingPool::bind_model(
            storage,
            SchedulerPoolModelAddress::new(0x2f00_0100).unwrap(),
            numbers,
        )
        .unwrap();
        let instance = pool.acquire().unwrap();
        pool.prepare_packet(&instance, &[0x02, 6, 1, 2, 3, 4, 5, 6])
            .unwrap();
        pool.reset_link_state(
            &instance,
            oer_esp32s31_bluetooth_memory::LeTxPower::from_dbm(0).expect("provider level"),
        )
        .unwrap();
        let event = pool
            .prepare_event(
                &instance,
                LegacyAdvertisingPrimaryChannelPlan::new(true, true, true).unwrap(),
                1_000,
                100,
                oer_esp32s31_bluetooth_memory::AdvertisingCoexistencePriorities {
                    lanes: [oer_esp32s31_bluetooth_memory::SchedulerItemCoexistencePriority::new(
                        15,
                    )
                    .unwrap(); 4],
                },
            )
            .unwrap();
        let items: Vec<(SchedulerItemId, SchedulerRawWindow)> = event
            .items()
            .map(|(item, start, end)| {
                (
                    pool.submit(&instance, item).unwrap(),
                    SchedulerRawWindow::new(start, end).unwrap(),
                )
            })
            .collect();

        let mut executor = SchedulerExecutor::<SchedulerItemId, 4>::new();
        let mut space = SchedulerItemSpace::new().with(&pool);
        // Submit out of order; the executor links them by start.
        let mut head = None;
        for (id, window) in items.iter().rev() {
            head = Some(
                executor
                    .submit_idle(&mut space, &idle(), *id, *window)
                    .unwrap()
                    .head,
            );
        }
        let order: Vec<SchedulerItemId> = executor.list().iter().map(|(id, _)| id).collect();
        assert_eq!(order, items.iter().map(|(id, _)| *id).collect::<Vec<_>>());
        assert_eq!(head, Some(space.link(items[0].0)));
    }
}
