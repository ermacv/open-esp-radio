use super::*;
mod observation;

fn us(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).unwrap()
}
fn candidate(key: u8, minimum: u32) -> AirtimeCandidate<u8> {
    AirtimeCandidate {
        key,
        minimum_micros: us(minimum),
    }
}

#[test]
fn active_and_standby_reserve_before_either_completion() {
    let mut storage = AirtimeStorage::<_, 2>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let eligible = [candidate(1, 100), candidate(2, 100)];
    let active = scheduler.reserve(eligible, &mut ()).unwrap().unwrap();
    let standby = scheduler.reserve(eligible, &mut ()).unwrap().unwrap();
    assert_eq!((active.key(), standby.key()), (1, 2));
    assert_eq!(scheduler.balance_micros(1), Some(0));
    assert_eq!(scheduler.balance_micros(2), Some(0));
    assert_eq!(
        scheduler.reserve(eligible, &mut ()),
        Err(AirtimeError::ReservationCapacity)
    );
    scheduler
        .settle(active.published().completed(()), us(1000), &mut ())
        .unwrap();
    let next = scheduler.reserve(eligible, &mut ()).unwrap().unwrap();
    assert_eq!(next.key(), 1);
    scheduler.cancel(next, &mut ()).unwrap();
    scheduler.cancel(standby, &mut ()).unwrap();
}

#[test]
fn retries_create_debt_and_unpublished_cancellation_refunds() {
    let mut storage = AirtimeStorage::<_, 2>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let eligible = [candidate(1, 1000), candidate(2, 1000)];
    let failed = scheduler.reserve(eligible, &mut ()).unwrap().unwrap();
    scheduler
        .settle(failed.published().completed(()), us(3500), &mut ())
        .unwrap();
    assert_eq!(scheduler.balance_micros(1), Some(-2500));
    // Four 1-ms services let the other peer catch up with 3.5 ms spent.
    for _ in 0..4 {
        let other = scheduler.reserve(eligible, &mut ()).unwrap().unwrap();
        assert_eq!(other.key(), 2);
        scheduler
            .settle(other.published().completed(()), us(1000), &mut ())
            .unwrap();
    }
    let pending = scheduler.reserve(eligible, &mut ()).unwrap().unwrap();
    assert_eq!(pending.key(), 1);
    assert_eq!(pending.budget_micros(), us(1500));
    scheduler.cancel(pending, &mut ()).unwrap();
    assert_eq!(scheduler.balance_micros(1), Some(1500));
}

#[test]
fn a_single_indebted_peer_advances_rounds_without_polling() {
    let mut storage = AirtimeStorage::<_, 1>::new(us(1));
    let mut scheduler = storage.scheduler();
    let only = [candidate(1, 1)];
    let first = scheduler.reserve(only, &mut ()).unwrap().unwrap();
    scheduler
        .settle(first.published().completed(()), us(u32::MAX), &mut ())
        .unwrap();
    let next = scheduler.reserve(only, &mut ()).unwrap().unwrap();
    assert_eq!(next.budget_micros(), us(1));
    scheduler.cancel(next, &mut ()).unwrap();
}

#[test]
fn minimum_exchange_larger_than_quantum_keeps_fractional_round_credit() {
    let mut storage = AirtimeStorage::<_, 2>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let eligible = [candidate(1, 2500), candidate(2, 1000)];
    let mut charged = [0u64; 2];
    for _ in 0..700 {
        let selected = scheduler.reserve(eligible, &mut ()).unwrap().unwrap();
        let index = usize::from(selected.key() - 1);
        let cost = if index == 0 { 2500 } else { 1000 };
        charged[index] += u64::from(cost);
        scheduler
            .settle(selected.published().completed(()), us(cost), &mut ())
            .unwrap();
    }
    assert!(charged[0].abs_diff(charged[1]) <= 3000, "{charged:?}");
}

#[test]
fn repeated_transport_demand_does_not_multiply_a_peers_credit() {
    let mut storage = AirtimeStorage::<_, 2>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let eligible = [
        candidate(1, 100),
        candidate(1, 500),
        candidate(2, 100),
        candidate(1, 100),
    ];
    for expected in [1, 2, 1, 2] {
        let reservation = scheduler.reserve(eligible, &mut ()).unwrap().unwrap();
        assert_eq!(reservation.key(), expected);
        assert_eq!(reservation.budget_micros(), us(1000));
        scheduler
            .settle(reservation.published().completed(()), us(1000), &mut ())
            .unwrap();
    }
}

#[test]
fn idle_peer_does_not_bank_credit_or_erase_retry_debt() {
    let mut storage = AirtimeStorage::<_, 2>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let both = [candidate(1, 100), candidate(2, 100)];
    let first = scheduler.reserve(both, &mut ()).unwrap().unwrap();
    scheduler
        .settle(first.published().completed(()), us(2500), &mut ())
        .unwrap();
    let other = scheduler
        .reserve([candidate(2, 100)], &mut ())
        .unwrap()
        .unwrap();
    assert_eq!(scheduler.balance_micros(1), Some(-1500));
    scheduler
        .settle(other.published().completed(()), us(100), &mut ())
        .unwrap();
    assert!(scheduler.reserve([], &mut ()).unwrap().is_none());
    assert_eq!(scheduler.balance_micros(2), Some(0));
    assert_eq!(scheduler.balance_micros(1), Some(-1500));
}

#[test]
fn late_completion_cannot_debit_a_new_association() {
    // A station slot key that carries its association generation.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct PeerKey {
        slot: u8,
        generation: u16,
    }
    let key = |generation| PeerKey {
        slot: 0,
        generation,
    };
    let candidate = |generation| AirtimeCandidate {
        key: key(generation),
        minimum_micros: us(100),
    };
    let mut storage = AirtimeStorage::<_, 2>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let old = scheduler.reserve([candidate(1)], &mut ()).unwrap().unwrap();
    scheduler.retire(key(1));
    assert_eq!(
        scheduler.reserve([candidate(1)], &mut ()),
        Err(AirtimeError::RetiredKey)
    );
    let new = scheduler.reserve([candidate(2)], &mut ()).unwrap().unwrap();
    scheduler
        .settle(old.published().completed(()), us(3000), &mut ())
        .unwrap();
    assert_eq!(scheduler.balance_micros(key(1)), None);
    assert_eq!(scheduler.balance_micros(key(2)), Some(0));
    scheduler
        .settle(new.published().completed(()), us(100), &mut ())
        .unwrap();
    assert_eq!(scheduler.balance_micros(key(2)), Some(900));
}

#[test]
fn capacity_and_counter_errors_preserve_the_outstanding_owner() {
    let mut storage = AirtimeStorage::<_, 1>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let first = scheduler
        .reserve([candidate(1, 100)], &mut ())
        .unwrap()
        .unwrap();
    assert_eq!(
        scheduler.reserve([candidate(2, 100)], &mut ()),
        Err(AirtimeError::AccountCapacity)
    );
    assert_eq!(scheduler.balance_micros(1), Some(0));
    scheduler.state.next_id = u64::MAX;
    assert_eq!(
        scheduler.reserve([candidate(1, 100)], &mut ()),
        Err(AirtimeError::ArithmeticOverflow)
    );
    assert_eq!(scheduler.balance_micros(1), Some(0));
    scheduler.cancel(first, &mut ()).unwrap();
    assert_eq!(scheduler.balance_micros(1), Some(1000));
}

#[test]
fn unknown_reservation_is_returned_without_charging_any_account() {
    let mut storage = AirtimeStorage::<_, 1>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let mut reservation = scheduler
        .reserve([candidate(1, 100)], &mut ())
        .unwrap()
        .unwrap();
    let actual_id = reservation.id;
    reservation.id += 1; // Deliberate internal fault injection; callers cannot forge this.
    let (error, mut completion) = scheduler
        .settle(reservation.published().completed(()), us(900), &mut ())
        .unwrap_err();
    assert_eq!(error, AirtimeError::UnknownReservation);
    assert_eq!(scheduler.balance_micros(1), Some(0));
    completion.publication.reservation.id = actual_id;
    scheduler.settle(completion, us(900), &mut ()).unwrap();
}

mod ownership;

#[test]
fn bulk_retirement_keeps_in_flight_old_generation_until_settled() {
    let mut storage = AirtimeStorage::<_, 3>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let old = scheduler
        .reserve([candidate(1, 100)], &mut ())
        .unwrap()
        .unwrap()
        .published();
    let current = scheduler
        .reserve([candidate(2, 100)], &mut ())
        .unwrap()
        .unwrap();
    scheduler.retire_where(|key| key != 2);
    assert_eq!(scheduler.balance_micros(1), Some(0));
    scheduler
        .settle(old.completed(()), us(1200), &mut ())
        .unwrap();
    assert_eq!(scheduler.balance_micros(1), None);
    scheduler.cancel(current, &mut ()).unwrap();
    assert_eq!(scheduler.balance_micros(2), Some(1000));
    let replacement = scheduler
        .reserve([candidate(3, 100)], &mut ())
        .unwrap()
        .unwrap();
    scheduler.cancel(replacement, &mut ()).unwrap();
}

#[test]
fn unfunded_internal_demand_returns_a_named_fault_with_its_cursor() {
    // Fault injection at the old `choose(...).expect(...)` boundary: no
    // funded owner is fabricated and the source account remains intact.
    let mut accounts = [Some(Account {
        key: 7,
        balance: 2,
        minimum: 3,
        retired: false,
    })];
    assert!(matches!(
        funded_account(&mut accounts, 0),
        Err(AirtimeError::UnfundedDemand { cursor: 0 })
    ));
    let account = accounts[0].unwrap();
    assert_eq!((account.key, account.balance, account.minimum), (7, 2, 3));
}

#[test]
fn arithmetic_boundaries_leave_ledgers_and_completion_owners_unchanged_on_error() {
    let mut storage = AirtimeStorage::<_, 2>::new(us(u32::MAX));
    let mut scheduler = storage.scheduler();
    let reservation = scheduler
        .reserve([candidate(1, u32::MAX)], &mut ())
        .unwrap()
        .unwrap();
    assert_eq!(reservation.budget_micros(), us(u32::MAX));
    scheduler.state.accounts[0].as_mut().unwrap().balance = i64::MAX;
    let (error, reservation) = scheduler.cancel(reservation, &mut ()).unwrap_err();
    assert_eq!(error, AirtimeError::ArithmeticOverflow);
    assert_eq!(scheduler.balance_micros(1), Some(i64::MAX));
    assert_eq!(scheduler.state.pending.iter().flatten().count(), 1);
    scheduler.state.accounts[0].as_mut().unwrap().balance = 0;
    scheduler.cancel(reservation, &mut ()).unwrap();
    scheduler.state.accounts[0].as_mut().unwrap().balance = i64::MIN;
    let before = scheduler.state.next_id;
    assert_eq!(
        scheduler.reserve([candidate(1, 1), candidate(2, 1)], &mut ()),
        Err(AirtimeError::ArithmeticOverflow)
    );
    assert_eq!(scheduler.state.next_id, before);
    assert_eq!(scheduler.balance_micros(1), Some(i64::MIN));
    assert_eq!(scheduler.balance_micros(2), None);
}
