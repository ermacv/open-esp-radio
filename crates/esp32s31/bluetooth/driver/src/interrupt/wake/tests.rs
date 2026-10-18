use super::{SchedulerWakeCell, SchedulerWakePublication, SchedulerWorkerWakeClass};

#[test]
fn first_publication_wakes_and_ordinary_duplicates_coalesce() {
    let cell = SchedulerWakeCell::new();

    assert_eq!(
        cell.publish_from_interrupt(SchedulerWorkerWakeClass::Ordinary),
        SchedulerWakePublication::WakeWorker
    );
    assert_eq!(
        cell.publish_from_interrupt(SchedulerWorkerWakeClass::Ordinary),
        SchedulerWakePublication::Coalesced
    );
    assert!(cell.is_pending());
    assert!(!cell.take().expect("one batch must be pending").is_marked());
    assert!(!cell.is_pending());
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
        let cell = SchedulerWakeCell::new();
        assert_eq!(
            cell.publish_from_interrupt(classes[0]),
            SchedulerWakePublication::WakeWorker
        );
        assert_eq!(
            cell.publish_from_interrupt(classes[1]),
            SchedulerWakePublication::Coalesced
        );
        assert!(cell.take().expect("one batch must be pending").is_marked());
    }
}

#[test]
fn dequeue_closes_the_epoch_and_the_next_publication_wakes_again() {
    let cell = SchedulerWakeCell::new();
    assert_eq!(
        cell.publish_from_interrupt(SchedulerWorkerWakeClass::Marked),
        SchedulerWakePublication::WakeWorker
    );
    assert!(
        cell.take()
            .expect("first batch must be pending")
            .is_marked()
    );
    assert_eq!(cell.take(), None);

    assert_eq!(
        cell.publish_from_interrupt(SchedulerWorkerWakeClass::Ordinary),
        SchedulerWakePublication::WakeWorker
    );
    assert!(
        !cell
            .take()
            .expect("second batch must be pending")
            .is_marked()
    );
}
