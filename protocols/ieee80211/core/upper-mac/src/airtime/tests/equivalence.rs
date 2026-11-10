//! The scheduler against the selection it had before it read its accounts
//! in place: the same decisions, results and ledger over any sequence of
//! reservations, settlements, cancellations and retirements. The reference
//! below is that earlier code, kept as it was, on its own copy.

extern crate std;

use std::borrow::ToOwned;
use std::format;
use std::string::String;
use std::vec::Vec;

use super::*;

/// What one reference selection decided, on its own copy of the accounts.
struct ReferenceSelection<K, const PEERS: usize> {
    accounts: [Option<Account<K>>; PEERS],
    selected: Option<usize>,
    place: usize,
}

fn reference_funded<K>(
    accounts: &mut [Option<Account<K>>],
    cursor: usize,
) -> Result<(usize, &mut Account<K>), AirtimeError> {
    accounts
        .iter_mut()
        .enumerate()
        .filter_map(|(index, slot)| slot.as_mut().map(|account| (index, account)))
        .filter(|(_, account)| {
            account.minimum != 0 && account.balance >= i64::from(account.minimum)
        })
        .min_by_key(|(index, _)| (*index < cursor, *index))
        .ok_or(AirtimeError::UnfundedDemand { cursor })
}

impl<'owner, K: Copy + Eq, const PEERS: usize, const RESERVATIONS: usize>
    AirtimeScheduler<'owner, K, PEERS, RESERVATIONS>
{
    fn reference_reserve_for(
        &mut self,
        wanted: impl FnOnce(K) -> bool,
        candidates: impl IntoIterator<Item = AirtimeCandidate<K>>,
    ) -> Result<Reserved<'owner, K>, AirtimeError> {
        let selection = self.reference_select(candidates)?;
        let key = selection.selected.and_then(|selected| {
            selection
                .accounts
                .get(selected)
                .copied()
                .flatten()
                .map(|account| account.key)
        });
        if let Some(key) = key
            && !wanted(key)
        {
            return Ok(Reserved::Other(key));
        }
        Ok(match self.reference_commit(selection, &mut ())? {
            Some(reservation) => Reserved::This(reservation),
            None => Reserved::Nothing,
        })
    }

    /// What [`Self::reserve`] decides over `candidates`, on a copy of the
    /// accounts: nothing changes until [`Self::commit`].
    fn reference_select(
        &self,
        candidates: impl IntoIterator<Item = AirtimeCandidate<K>>,
    ) -> Result<ReferenceSelection<K, PEERS>, AirtimeError> {
        let place = self
            .state
            .vacant()
            .ok_or(AirtimeError::ReservationCapacity)?;
        // Admission is transactional: this bounded copy owns no packets.
        let mut accounts = self.state.accounts;
        for account in accounts.iter_mut().flatten() {
            account.minimum = 0;
        }
        for candidate in candidates {
            if let Some(account) = accounts
                .iter_mut()
                .flatten()
                .find(|a| a.key == candidate.key)
            {
                if account.retired {
                    return Err(AirtimeError::RetiredKey);
                }
                account.minimum = account.minimum.max(candidate.minimum_micros.get());
            } else {
                let slot = accounts
                    .iter_mut()
                    .find(|slot| slot.is_none())
                    .ok_or(AirtimeError::AccountCapacity)?;
                *slot = Some(Account {
                    key: candidate.key,
                    balance: 0,
                    minimum: candidate.minimum_micros.get(),
                    retired: false,
                    outstanding: 0,
                    outstanding_micros: 0,
                });
            }
        }
        for account in accounts.iter_mut().flatten() {
            if account.minimum == 0 {
                account.balance = account.balance.min(0);
            }
        }
        if !accounts
            .iter()
            .flatten()
            .any(|a| a.minimum != 0 && a.balance >= i64::from(a.minimum))
        {
            let quantum = i64::from(self.state.quantum.get());
            let mut rounds: Option<i64> = None;
            for account in accounts.iter().flatten().filter(|a| a.minimum != 0) {
                let required = i64::from(account.minimum)
                    .checked_sub(account.balance)
                    .and_then(|needed| needed.checked_sub(1))
                    .and_then(|needed| needed.checked_div(quantum))
                    .and_then(|rounds| rounds.checked_add(1))
                    .ok_or(AirtimeError::ArithmeticOverflow)?;
                rounds = Some(rounds.map_or(required, |old| old.min(required)));
            }
            let Some(rounds) = rounds else {
                return Ok(ReferenceSelection {
                    accounts,
                    selected: None,
                    place,
                });
            };
            let credit = quantum
                .checked_mul(rounds)
                .ok_or(AirtimeError::ArithmeticOverflow)?;
            for account in accounts.iter_mut().flatten().filter(|a| a.minimum != 0) {
                account.balance = account
                    .balance
                    .checked_add(credit)
                    .ok_or(AirtimeError::ArithmeticOverflow)?;
            }
        }
        let (selected, _) = reference_funded(&mut accounts, self.state.cursor)?;
        Ok(ReferenceSelection {
            accounts,
            selected: Some(selected),
            place,
        })
    }

    /// Make `selection` the ledger's: its reservation, or, with none
    /// selected, its accounts as the selection left them.
    fn reference_commit(
        &mut self,
        selection: ReferenceSelection<K, PEERS>,
        events: &mut impl EventSink<AirtimeObservation<K>>,
    ) -> Result<Option<AirtimeReservation<'owner, K>>, AirtimeError> {
        let ReferenceSelection {
            mut accounts,
            selected,
            place,
        } = selection;
        let Some(selected) = selected else {
            self.state.accounts = accounts;
            return Ok(None);
        };
        let account = accounts.get_mut(selected).and_then(Option::as_mut).ok_or(
            AirtimeError::UnfundedDemand {
                cursor: self.state.cursor,
            },
        )?;
        let budget = u32::try_from(account.balance.min(i64::from(u32::MAX)))
            .ok()
            .and_then(NonZeroU32::new)
            .ok_or(AirtimeError::ArithmeticOverflow)?;
        let id = self
            .state
            .next_id
            .checked_add(1)
            .ok_or(AirtimeError::ArithmeticOverflow)?;
        let reservation = AirtimeReservation {
            origin: self.origin,
            key: account.key,
            id,
            budget,
            place,
        };
        account.balance = account
            .balance
            .checked_sub(i64::from(budget.get()))
            .ok_or(AirtimeError::ArithmeticOverflow)?;
        // At most `RESERVATIONS` reservations of u32 budgets, which the
        // storage's const bound keeps within both counts.
        account.outstanding = account.outstanding.saturating_add(1);
        account.outstanding_micros = account
            .outstanding_micros
            .saturating_add(u64::from(budget.get()));
        let observed = *account;
        // The cursor wraps at the bounded peer arena, rather than at usize::MAX.
        let cursor = selected
            .wrapping_add(1)
            .checked_rem(PEERS)
            .ok_or(AirtimeError::ArithmeticOverflow)?;
        let Some(pending) = self.state.pending.get_mut(place) else {
            return Err(AirtimeError::ReservationCapacity);
        };
        *pending = Some(Pending {
            id,
            key: observed.key,
            budget: budget.get(),
        });
        self.state.occupy();
        self.state.accounts = accounts;
        self.state.next_id = id;
        self.state.cursor = cursor;
        self.observe(observed, AirtimeAction::Granted, budget.get(), 0, events);
        Ok(Some(reservation))
    }
}

/// A ledger as values, to compare.
type Ledger = (
    Vec<Option<(u8, i64, u32, bool, u32, u64)>>,
    Vec<Option<(u64, u8, u32)>>,
    Vec<usize>,
    usize,
    usize,
    usize,
    u64,
);

fn ledger<const PEERS: usize, const RESERVATIONS: usize>(
    state: &State<u8, PEERS, RESERVATIONS>,
) -> Ledger {
    (
        state
            .accounts
            .iter()
            .map(|slot| {
                slot.map(|a| {
                    (
                        a.key,
                        a.balance,
                        a.minimum,
                        a.retired,
                        a.outstanding,
                        a.outstanding_micros,
                    )
                })
            })
            .collect(),
        state
            .pending
            .iter()
            .map(|slot| slot.map(|p| (p.id, p.key, p.budget)))
            .collect(),
        state.free.to_vec(),
        state.freed,
        state.unused,
        state.cursor,
        state.next_id,
    )
}

/// What a reservation result says, beside its owner.
fn said(result: &Result<Reserved<'_, u8>, AirtimeError>) -> String {
    match result {
        Ok(Reserved::This(reservation)) => format!(
            "this {} {} {} {}",
            reservation.key, reservation.id, reservation.budget, reservation.place
        ),
        Ok(Reserved::Nothing) => "nothing".to_owned(),
        Ok(Reserved::Other(key)) => format!("other {key}"),
        Err(error) => format!("{error:?}"),
    }
}

struct Random(u64);

impl Random {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % below
    }
}

#[test]
fn the_scheduler_decides_as_its_copying_reference_over_any_sequence() {
    for seed in 0..300_u64 {
        let mut random = Random(seed);
        let quantum = us(1 + random.next(3000) as u32);
        let mut ours = AirtimeStorage::<u8, 4, 6>::new(quantum);
        let mut theirs = AirtimeStorage::<u8, 4, 6>::new(quantum);
        let mut ours_held = Vec::new();
        let mut theirs_held = Vec::new();
        let mut a = ours.scheduler();
        let mut b = theirs.scheduler();
        for step in 0..200 {
            let operation = random.next(10);
            {
                match operation {
                    0..=5 => {
                        let count = random.next(4);
                        let candidates: Vec<_> = (0..count)
                            .map(|_| candidate(random.next(6) as u8, 1 + random.next(4000) as u32))
                            .collect();
                        let wanted = random.next(7);
                        let take = |key: u8| wanted == 6 || u64::from(key) == wanted;
                        let ours_result = a.reserve_for(take, candidates.clone(), &mut ());
                        let theirs_result = b.reference_reserve_for(take, candidates.clone());
                        assert_eq!(
                            said(&ours_result),
                            said(&theirs_result),
                            "seed {seed} step {step}: {candidates:?}"
                        );
                        if let (Ok(Reserved::This(x)), Ok(Reserved::This(y))) =
                            (ours_result, theirs_result)
                        {
                            ours_held.push(x);
                            theirs_held.push(y);
                        }
                    }
                    6 | 7 if !ours_held.is_empty() => {
                        let at = random.next(ours_held.len() as u64) as usize;
                        let charged = us(1 + random.next(6000) as u32);
                        let x = ours_held.swap_remove(at);
                        let y = theirs_held.swap_remove(at);
                        let ours_end = a
                            .settle(x.published().completed(()), charged, &mut ())
                            .map_err(|(error, _)| error);
                        let theirs_end = b
                            .settle(y.published().completed(()), charged, &mut ())
                            .map_err(|(error, _)| error);
                        assert_eq!(ours_end, theirs_end, "seed {seed} step {step}");
                    }
                    8 if !ours_held.is_empty() => {
                        let at = random.next(ours_held.len() as u64) as usize;
                        let x = ours_held.swap_remove(at);
                        let y = theirs_held.swap_remove(at);
                        let ours_end = a.cancel(x, &mut ()).map_err(|(error, _)| error);
                        let theirs_end = b.cancel(y, &mut ()).map_err(|(error, _)| error);
                        assert_eq!(ours_end, theirs_end, "seed {seed} step {step}");
                    }
                    _ => {
                        let key = random.next(6) as u8;
                        a.retire(key);
                        b.retire(key);
                    }
                }
            }
            assert_eq!(ledger(a.state), ledger(b.state), "seed {seed} step {step}");
        }
    }
}

/// A reservation over one candidate decides as the selection over that one
/// candidate: the same result, reservation and ledger, over any sequence of
/// reservations, settlements, cancellations and retirements, every account
/// and the horizon full among them.
#[test]
fn a_reservation_over_one_candidate_decides_as_the_selection_over_it() {
    fn told(result: &Result<Option<AirtimeReservation<'_, u8>>, AirtimeError>) -> String {
        match result {
            Ok(Some(reservation)) => format!(
                "this {} {} {} {}",
                reservation.key, reservation.id, reservation.budget, reservation.place
            ),
            Ok(None) => "nothing".to_owned(),
            Err(error) => format!("{error:?}"),
        }
    }
    for seed in 0..300_u64 {
        let mut random = Random(seed);
        let quantum = us(1 + random.next(3000) as u32);
        let mut ours = AirtimeStorage::<u8, 4, 6>::new(quantum);
        let mut theirs = AirtimeStorage::<u8, 4, 6>::new(quantum);
        let mut ours_held = Vec::new();
        let mut theirs_held = Vec::new();
        let mut a = ours.scheduler();
        let mut b = theirs.scheduler();
        for step in 0..200 {
            match random.next(10) {
                0..=5 => {
                    let one = candidate(random.next(6) as u8, 1 + random.next(9000) as u32);
                    let ours_result = a.reserve_one(one, &mut ());
                    let theirs_result = b.reserve([one], &mut ());
                    assert_eq!(
                        told(&ours_result),
                        told(&theirs_result),
                        "seed {seed} step {step}: {one:?}"
                    );
                    if let (Ok(Some(x)), Ok(Some(y))) = (ours_result, theirs_result) {
                        ours_held.push(x);
                        theirs_held.push(y);
                    }
                }
                6 | 7 if !ours_held.is_empty() => {
                    let at = random.next(ours_held.len() as u64) as usize;
                    let charged = us(1 + random.next(6000) as u32);
                    let x = ours_held.swap_remove(at);
                    let y = theirs_held.swap_remove(at);
                    let ours_end = a
                        .settle(x.published().completed(()), charged, &mut ())
                        .map_err(|(error, _)| error);
                    let theirs_end = b
                        .settle(y.published().completed(()), charged, &mut ())
                        .map_err(|(error, _)| error);
                    assert_eq!(ours_end, theirs_end, "seed {seed} step {step}");
                }
                8 if !ours_held.is_empty() => {
                    let at = random.next(ours_held.len() as u64) as usize;
                    let x = ours_held.swap_remove(at);
                    let y = theirs_held.swap_remove(at);
                    let ours_end = a.cancel(x, &mut ()).map_err(|(error, _)| error);
                    let theirs_end = b.cancel(y, &mut ()).map_err(|(error, _)| error);
                    assert_eq!(ours_end, theirs_end, "seed {seed} step {step}");
                }
                _ => {
                    let key = random.next(6) as u8;
                    a.retire(key);
                    b.retire(key);
                }
            }
            assert_eq!(ledger(a.state), ledger(b.state), "seed {seed} step {step}");
        }
    }
}
