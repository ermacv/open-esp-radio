extern crate std;
use super::*;
use std::vec::Vec;

/// The observations a test's calls reported, in order.
#[derive(Default)]
struct Kept(Vec<AirtimeObservation<u8>>);

impl EventSink<AirtimeObservation<u8>> for Kept {
    fn event(&mut self, event: AirtimeObservation<u8>) {
        self.0.push(event);
    }
}

#[test]
fn observations_follow_committed_edges_and_preserve_unresolved_reservations() {
    let mut events = Kept::default();
    let mut storage = AirtimeStorage::<_, 1>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let first = scheduler
        .reserve([candidate(1, 100)], &mut events)
        .unwrap()
        .unwrap();
    let second = scheduler
        .reserve([candidate(1, 100)], &mut events)
        .unwrap()
        .unwrap();
    assert_eq!(
        scheduler.reserve([candidate(1, 100)], &mut events),
        Err(AirtimeError::ReservationCapacity)
    );
    scheduler
        .settle(first.published().completed(()), us(1500), &mut events)
        .unwrap();
    scheduler.retire(1);
    scheduler.cancel(second, &mut events).unwrap();
    let events = events.0;
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
    assert_eq!(scheduler.balance_micros(1), None);
}

#[test]
fn failed_cross_ledger_settlement_emits_nothing_and_drop_does_not_refund() {
    let mut events = Kept::default();
    let mut storage = AirtimeStorage::<_, 1>::new(us(1000));
    let mut other = AirtimeStorage::<_, 1>::new(us(1000));
    let reservation = storage
        .scheduler()
        .reserve([candidate(1, 100)], &mut events)
        .unwrap()
        .unwrap();
    let (_, completion) = other
        .scheduler()
        .settle(reservation.published().completed(()), us(100), &mut events)
        .unwrap_err();
    drop(completion);
    assert_eq!(storage.scheduler().balance_micros(1), Some(0));
    assert_eq!(events.0.len(), 1);
}
