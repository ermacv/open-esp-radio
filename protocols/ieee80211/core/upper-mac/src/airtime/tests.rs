use super::*;
mod equivalence;
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
    // An account held at place 0 whose balance falls short of its least
    // grant is not funded: the choice names the fault and its cursor.
    assert_eq!(
        funded_place(&[3], &[2], &[true], 0),
        Err(AirtimeError::UnfundedDemand { cursor: 0 })
    );
    assert_eq!(funded_place(&[3], &[3], &[true], 0), Ok(0));
    assert_eq!(
        funded_place(&[3], &[3], &[false], 0),
        Err(AirtimeError::UnfundedDemand { cursor: 0 })
    );
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

/// Each reservation reads and frees its own place: the horizon holds
/// exactly `RESERVATIONS`, a place an end frees serves the next one,
/// whichever ended, and a refused reservation takes no place.
#[test]
fn reservations_reuse_the_places_their_ends_free_and_a_refusal_takes_none() {
    const RESERVATIONS: usize = 3;
    let mut storage = AirtimeStorage::<_, 1, RESERVATIONS>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let mut held: std::vec::Vec<_> = (0..RESERVATIONS)
        .map(|_| {
            scheduler
                .reserve([candidate(1, 100)], &mut ())
                .unwrap()
                .unwrap()
        })
        .collect();
    assert_eq!(
        scheduler.reserve([candidate(1, 100)], &mut ()),
        Err(AirtimeError::ReservationCapacity)
    );
    // Another account than the one storage holds is refused whole.
    assert_eq!(
        scheduler.reserve([candidate(2, 100)], &mut ()),
        Err(AirtimeError::ReservationCapacity)
    );
    // The middle one ends: its place takes the next, and only one.
    let middle = held.remove(1);
    scheduler
        .settle(middle.published().completed(()), us(100), &mut ())
        .unwrap();
    let next = scheduler
        .reserve([candidate(1, 100)], &mut ())
        .unwrap()
        .unwrap();
    assert_eq!(
        scheduler.reserve([candidate(1, 100)], &mut ()),
        Err(AirtimeError::ReservationCapacity)
    );
    held.push(next);
    // Ended in any order, each is known at its place, once.
    for reservation in held.into_iter().rev() {
        scheduler.cancel(reservation, &mut ()).unwrap();
    }
    assert_eq!(scheduler.state.pending.iter().flatten().count(), 0);
    // Many rounds over few places: nothing leaks.
    for _ in 0..100 {
        let reservation = scheduler
            .reserve([candidate(1, 100)], &mut ())
            .unwrap()
            .unwrap();
        scheduler
            .settle(reservation.published().completed(()), us(100), &mut ())
            .unwrap();
    }
    let all: std::vec::Vec<_> = (0..RESERVATIONS)
        .map(|_| {
            scheduler
                .reserve([candidate(1, 100)], &mut ())
                .unwrap()
                .unwrap()
        })
        .collect();
    assert_eq!(all.len(), RESERVATIONS);
    for reservation in all {
        scheduler.cancel(reservation, &mut ()).unwrap();
    }
}

/// What an observation counts of an account's reservations follows each
/// grant and end, and a retired account goes with its last.
#[test]
fn an_account_counts_its_outstanding_reservations_and_goes_with_the_last() {
    #[derive(Default)]
    struct Seen(std::vec::Vec<(u32, u64)>);
    impl EventSink<AirtimeObservation<u8>> for Seen {
        fn event(&mut self, event: AirtimeObservation<u8>) {
            self.0.push((event.outstanding, event.outstanding_micros));
        }
    }
    let mut storage = AirtimeStorage::<_, 2, 4>::new(us(300));
    let mut scheduler = storage.scheduler();
    let mut seen = Seen::default();
    let first = scheduler
        .reserve([candidate(1, 100)], &mut seen)
        .unwrap()
        .unwrap();
    let second = scheduler
        .reserve([candidate(1, 100)], &mut seen)
        .unwrap()
        .unwrap();
    let (a, b) = (first.budget_micros().get(), second.budget_micros().get());
    assert_eq!(
        seen.0,
        [(1, u64::from(a)), (2, u64::from(a) + u64::from(b))]
    );
    scheduler.retire(1);
    scheduler
        .settle(first.published().completed(()), us(50), &mut seen)
        .unwrap();
    assert_eq!(seen.0.last(), Some(&(1, u64::from(b))));
    assert!(scheduler.balance_micros(1).is_some(), "one still pins it");
    scheduler.cancel(second, &mut seen).unwrap();
    assert_eq!(seen.0.last(), Some(&(0, 0)));
    assert_eq!(scheduler.balance_micros(1), None, "gone with its last");
}

/// A look at the next grant changes nothing, and a reservation for another
/// key than the one it would grant changes nothing either: the grant that
/// follows is the one a plain reservation would have made, round credit and
/// cursor alike.
#[test]
fn a_look_or_an_unwanted_grant_changes_nothing() {
    let eligible = [candidate(1, 1500), candidate(2, 700)];
    let mut looked = AirtimeStorage::<_, 2>::new(us(1000));
    let mut plain = AirtimeStorage::<_, 2>::new(us(1000));
    let mut looked = looked.scheduler();
    let mut plain = plain.scheduler();
    for _ in 0..6 {
        let key = looked.selects(eligible).unwrap().unwrap();
        assert_eq!(
            looked.selects(eligible),
            Ok(Some(key)),
            "a look is no grant"
        );
        assert_eq!(
            looked.reserve_for(|wanted| wanted != key, eligible, &mut ()),
            Ok(Reserved::Other(key)),
            "an unwanted grant is told, not made"
        );
        let Reserved::This(mine) = looked
            .reserve_for(|wanted| wanted == key, eligible, &mut ())
            .unwrap()
        else {
            panic!("the wanted key is granted");
        };
        let theirs = plain.reserve(eligible, &mut ()).unwrap().unwrap();
        assert_eq!(
            (mine.key(), mine.budget_micros()),
            (theirs.key(), theirs.budget_micros())
        );
        for key in [1, 2] {
            assert_eq!(looked.balance_micros(key), plain.balance_micros(key));
        }
        looked
            .settle(mine.published().completed(()), us(900), &mut ())
            .unwrap();
        plain
            .settle(theirs.published().completed(()), us(900), &mut ())
            .unwrap();
    }
}

/// With nothing to grant, `reserve_for` leaves the accounts as `reserve`
/// does: an idle account loses its surplus either way.
#[test]
fn reserve_for_with_nothing_to_grant_is_reserve() {
    let mut storage = AirtimeStorage::<_, 2>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let first = scheduler
        .reserve([candidate(1, 100)], &mut ())
        .unwrap()
        .unwrap();
    scheduler
        .settle(first.published().completed(()), us(100), &mut ())
        .unwrap();
    assert_eq!(scheduler.balance_micros(1), Some(900));
    assert_eq!(scheduler.selects([]), Ok(None));
    assert_eq!(scheduler.balance_micros(1), Some(900), "a look keeps it");
    assert_eq!(
        scheduler.reserve_for(|_| true, [], &mut ()),
        Ok(Reserved::Nothing)
    );
    assert_eq!(scheduler.balance_micros(1), Some(0), "idle loses surplus");
}
