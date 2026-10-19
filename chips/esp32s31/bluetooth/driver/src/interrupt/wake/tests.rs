use core::task::{Context, Poll, Waker};

use oer_interrupt_table::signal::{ForeignContext, host};

use super::{SchedulerWakeCell, SchedulerWakePublication, SchedulerWorkerWakeClass};

fn cell() -> SchedulerWakeCell {
    SchedulerWakeCell::new(host::deferred_signal())
}

#[test]
fn first_publication_pends_and_ordinary_duplicates_coalesce() {
    let cell = cell();

    assert_eq!(
        cell.publish_from_interrupt(SchedulerWorkerWakeClass::Ordinary),
        SchedulerWakePublication::Pended
    );
    assert_eq!(
        cell.publish_from_interrupt(SchedulerWorkerWakeClass::Ordinary),
        SchedulerWakePublication::Coalesced
    );
    assert_eq!(cell.is_pending(), Ok(true));
    assert!(
        !cell
            .take()
            .expect("the host executor's context")
            .expect("one batch must be pending")
            .is_marked()
    );
    assert_eq!(cell.is_pending(), Ok(false));
}

#[test]
fn marker_is_sticky_for_both_publication_orders() {
    for classes in [
        [
            SchedulerWorkerWakeClass::Ordinary,
            SchedulerWorkerWakeClass::Marked,
        ],
        [
            SchedulerWorkerWakeClass::Marked,
            SchedulerWorkerWakeClass::Ordinary,
        ],
    ] {
        let cell = cell();
        assert_eq!(
            cell.publish_from_interrupt(classes[0]),
            SchedulerWakePublication::Pended
        );
        assert_eq!(
            cell.publish_from_interrupt(classes[1]),
            SchedulerWakePublication::Coalesced
        );
        assert!(
            cell.take()
                .expect("the host executor's context")
                .expect("one batch must be pending")
                .is_marked()
        );
    }
}

#[test]
fn dequeue_closes_the_epoch_and_the_next_publication_pends_again() {
    let cell = cell();
    assert_eq!(
        cell.publish_from_interrupt(SchedulerWorkerWakeClass::Marked),
        SchedulerWakePublication::Pended
    );
    assert!(
        cell.take()
            .expect("the host executor's context")
            .expect("first batch must be pending")
            .is_marked()
    );
    assert_eq!(cell.take(), Ok(None));

    assert_eq!(
        cell.publish_from_interrupt(SchedulerWorkerWakeClass::Ordinary),
        SchedulerWakePublication::Pended
    );
    assert!(
        !cell
            .take()
            .expect("the host executor's context")
            .expect("second batch must be pending")
            .is_marked()
    );
}

#[test]
fn the_worker_waits_for_a_batch_in_its_executor_only() {
    let cell = SchedulerWakeCell::new(host::signal());
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(cell.poll_take(&mut cx), Poll::Pending);
    let _ = cell.publish_from_interrupt(SchedulerWorkerWakeClass::Marked);
    match cell.poll_take(&mut cx) {
        Poll::Ready(Ok(batch)) => assert!(batch.is_marked()),
        other => panic!("one marked batch must be ready: {other:?}"),
    }
    assert_eq!(cell.poll_take(&mut cx), Poll::Pending);

    let foreign = SchedulerWakeCell::new(host::foreign_signal());
    assert_eq!(
        foreign.publish_from_interrupt(SchedulerWorkerWakeClass::Ordinary),
        SchedulerWakePublication::Pended
    );
    assert_eq!(foreign.poll_take(&mut cx), Poll::Ready(Err(ForeignContext)));
    assert_eq!(foreign.take(), Err(ForeignContext));
}
