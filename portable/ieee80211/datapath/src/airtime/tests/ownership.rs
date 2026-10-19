use super::*;
use core::cell::Cell;
use oer_ieee80211_softmac::MacTxWork;

#[test]
fn identical_reservations_from_independent_ledgers_cannot_cross_cancel_or_settle() {
    let mut first_storage = AirtimeStorage::<_, 1, 1>::new(us(1000));
    let mut second_storage = AirtimeStorage::<_, 1, 1>::new(us(1000));
    let mut first = first_storage.scheduler();
    let mut second = second_storage.scheduler();
    let original = first.reserve([candidate(1, 100)]).unwrap().unwrap();
    let other = second.reserve([candidate(1, 100)]).unwrap().unwrap();
    // Public scheduling inputs and local serial numbers deliberately coincide.
    assert_eq!(
        (original.key(), original.id, original.budget_micros()),
        (other.key(), other.id, other.budget_micros())
    );
    assert_ne!(original, other);
    let (error, original) = second.cancel(original).unwrap_err();
    assert_eq!(error, AirtimeError::WrongScheduler);
    assert_eq!(first.balance_micros(1), Some(0));
    assert_eq!(second.balance_micros(1), Some(0));

    #[derive(Debug)]
    struct Receipt<'a>(&'a Cell<usize>);
    impl Drop for Receipt<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let drops = Cell::new(0);
    let completion = original.published().completed(Receipt(&drops));
    let (error, completion) = second.settle(completion, us(1400)).unwrap_err();
    assert_eq!(error, AirtimeError::WrongScheduler);
    assert_eq!(
        drops.get(),
        0,
        "a rejected settlement preserves the exact receipt"
    );
    assert!(matches!(
        second.reserve([]),
        Err(AirtimeError::ReservationCapacity)
    ));
    let receipt = first.settle(completion, us(1400)).unwrap();
    assert_eq!(first.balance_micros(1), Some(-400));
    assert_eq!(second.balance_micros(1), Some(0));
    assert_eq!(drops.get(), 0);
    drop(receipt);
    assert_eq!(drops.get(), 1);
    second.cancel(other).unwrap();
    assert_eq!(second.balance_micros(1), Some(1000));
}

#[test]
fn moving_the_scheduler_handle_preserves_reservation_identity() {
    let mut storage = AirtimeStorage::<_, 1>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let reservation = scheduler.reserve([candidate(1, 100)]).unwrap().unwrap();
    let mut moved = core::hint::black_box(scheduler);
    moved.cancel(reservation).unwrap();
    assert_eq!(moved.balance_micros(1), Some(1000));
}

#[test]
fn dropping_unsettled_work_or_reborrowing_storage_cannot_refund_it() {
    let mut storage = AirtimeStorage::<_, 1, 1>::new(us(1000));
    {
        let mut scheduler = storage.scheduler();
        let reservation = scheduler.reserve([candidate(1, 100)]).unwrap().unwrap();
        let _lost = reservation.published().completed(());
        // The owner can be lost through ordinary Rust drop, but never converted
        // into fresh credit. Recovery requires an explicit radio teardown policy.
    }
    let mut scheduler = storage.scheduler();
    assert_eq!(scheduler.balance_micros(1), Some(0));
    assert!(matches!(
        scheduler.reserve([candidate(1, 100)]),
        Err(AirtimeError::ReservationCapacity)
    ));
}

#[test]
fn unknown_receipt_keeps_the_reservation_until_an_explicit_charge_is_supplied() {
    let mut storage = AirtimeStorage::<_, 1, 1>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let reservation = scheduler.reserve([candidate(1, 100)]).unwrap().unwrap();
    let mut receipt = MacTxWork::new();
    receipt.record(1000, 1, None);
    let completion = reservation.published().completed(receipt);
    assert_eq!(completion.key(), 1);
    assert_eq!(completion.reserved_micros(), us(1000));
    assert!(completion.work().estimated_exchange_micros(0).is_none());
    assert!(matches!(
        scheduler.reserve([]),
        Err(AirtimeError::ReservationCapacity)
    ));
    assert_eq!(scheduler.balance_micros(1), Some(0));
    // Synthetic caller policy for this ownership test, not a PHY estimate.
    let retained = scheduler.settle(completion, us(1700)).unwrap();
    assert_eq!(retained, receipt);
    assert_eq!(scheduler.balance_micros(1), Some(-700));
}
