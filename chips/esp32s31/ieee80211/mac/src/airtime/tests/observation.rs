extern crate std;
use super::*;
use std::{cell::RefCell, vec::Vec};

std::thread_local! {
    static EVENTS: RefCell<Vec<AirtimeObservation<u8>>> = const { RefCell::new(Vec::new()) };
}

fn observe(event: AirtimeObservation<u8>) {
    EVENTS.with(|events| events.borrow_mut().push(event));
}

#[test]
fn observations_follow_committed_edges_and_preserve_unresolved_reservations() {
    EVENTS.with(|e| e.borrow_mut().clear());
    let mut storage = AirtimeStorage::<_, 1>::new(us(1000)).with_observer(Some(observe));
    let mut scheduler = storage.scheduler();
    let first = scheduler.reserve([candidate(1, 100)]).unwrap().unwrap();
    let second = scheduler.reserve([candidate(1, 100)]).unwrap().unwrap();
    assert_eq!(
        scheduler.reserve([candidate(1, 100)]),
        Err(AirtimeError::ReservationCapacity)
    );
    scheduler
        .settle(first.published().completed(()), us(1500))
        .unwrap();
    scheduler.retire(1);
    scheduler.cancel(second).unwrap();
    EVENTS.with(|events| {
        let events = events.borrow();
        assert_eq!(events.len(), 4);
        assert_eq!(events[1].outstanding, 2);
        assert_eq!(events[1].outstanding_micros, 2000);
        assert_eq!(events[2].action, AirtimeAction::Settled);
        assert_eq!(events[2].charged_micros, 1500);
        assert_eq!(events[2].balance_after_event_micros, -500);
        assert_eq!(events[2].outstanding, 1);
        assert_eq!(events[3].action, AirtimeAction::Cancelled);
        assert_eq!(events[3].charged_micros, 0);
        assert_eq!(events[3].outstanding, 0);
        assert_eq!(events[3].outstanding_micros, 0);
    });
    assert_eq!(scheduler.balance_micros(1), None);
}

#[test]
fn failed_cross_ledger_settlement_emits_nothing_and_drop_does_not_refund() {
    EVENTS.with(|e| e.borrow_mut().clear());
    let mut storage = AirtimeStorage::<_, 1>::new(us(1000)).with_observer(Some(observe));
    let mut other = AirtimeStorage::<_, 1>::new(us(1000)).with_observer(Some(observe));
    let reservation = storage
        .scheduler()
        .reserve([candidate(1, 100)])
        .unwrap()
        .unwrap();
    let (_, completion) = other
        .scheduler()
        .settle(reservation.published().completed(()), us(100))
        .unwrap_err();
    drop(completion);
    assert_eq!(storage.scheduler().balance_micros(1), Some(0));
    EVENTS.with(|e| assert_eq!(e.borrow().len(), 1));
}
