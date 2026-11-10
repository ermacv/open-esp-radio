//! Bounded deficit scheduling of radio peers, independent of packet storage.
//!
//! Reserve before preparing hardware work, settle after terminal completion,
//! and cancel only unpublished work. Active and standby batches share this
//! account. Keys must include the radio association generation, but not a
//! transport flow identity. All methods are synchronous and allocation-free.

use core::num::NonZeroU32;
use oer_portable_events::EventSink;

mod observation;
pub use observation::{AirtimeAction, AirtimeObservation};

/// One currently eligible outer queue and the minimum useful exchange grant.
/// Repeated keys do not earn extra rounds; their largest minimum is retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AirtimeCandidate<K> {
    pub key: K,
    pub minimum_micros: NonZeroU32,
}

#[derive(Clone, Copy)]
struct Account<K> {
    key: K,
    balance: i64,
    minimum: u32,
    retired: bool,
    /// Its reservations not yet settled or cancelled, and their budgets:
    /// a retired account goes when the count reaches zero.
    outstanding: u32,
    outstanding_micros: u64,
}

#[derive(Clone, Copy)]
struct Pending<K> {
    id: u64,
    key: K,
    budget: u32,
}

// A shared borrow pins this nonzero-sized identity in its storage. Comparing
// addresses distinguishes independent ledgers even when every value matches.
#[derive(Clone, Copy, Debug)]
struct Origin<'owner>(&'owner u8);

impl PartialEq for Origin<'_> {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.0, other.0)
    }
}
impl Eq for Origin<'_> {}

/// An affine reservation from one scheduler. Return it to that same owner.
/// Dropping it does not refund credits: outstanding preparation/publication
/// must first be explicitly cancelled or settled. No payload or DMA is owned.
#[must_use = "settle a published reservation or cancel unpublished work"]
#[derive(Debug, Eq, PartialEq)]
pub struct AirtimeReservation<'owner, K> {
    origin: Origin<'owner>,
    key: K,
    id: u64,
    budget: NonZeroU32,
    /// Its place among the scheduler's reservations, which its end reads
    /// directly; the id tells a later reservation of the place apart.
    place: usize,
}

impl<'owner, K: Copy> AirtimeReservation<'owner, K> {
    pub const fn key(&self) -> K {
        self.key
    }

    pub const fn budget_micros(&self) -> NonZeroU32 {
        self.budget
    }

    /// Cross the successful hardware-publication edge. Subsequent retries
    /// retain this one charge owner; cancellation can no longer refund it.
    pub fn published(self) -> AirtimeInFlight<'owner, K> {
        AirtimeInFlight { reservation: self }
    }
}

/// Published work must be settled, including terminal abort or timeout.
/// It cannot use the unpublished-reservation cancellation API.
/// ```compile_fail,E0308
/// use core::num::NonZeroU32;
/// use oer_protocols_ieee80211_core_upper_mac::airtime::{AirtimeStorage, AirtimeCandidate};
/// let us = NonZeroU32::new(100).unwrap();
/// let mut storage = AirtimeStorage::<_, 1>::new(us);
/// let mut scheduler = storage.scheduler();
/// let reservation = scheduler.reserve([AirtimeCandidate { key: 1, minimum_micros: us }], &mut ())
///     .unwrap().unwrap();
/// scheduler.cancel(reservation.published(), &mut ());
/// ```
#[must_use = "settle committed transmission work after terminal release"]
#[derive(Debug, Eq, PartialEq)]
pub struct AirtimeInFlight<'owner, K> {
    reservation: AirtimeReservation<'owner, K>,
}

impl<'owner, K: Copy> AirtimeInFlight<'owner, K> {
    pub const fn key(&self) -> K {
        self.reservation.key()
    }

    pub const fn reserved_micros(&self) -> NonZeroU32 {
        self.reservation.budget_micros()
    }

    /// Retain the final receipt after all publications and terminal hardware
    /// detach. Retrying does not call this: keep the same in-flight owner.
    /// Physical storage can then be released independently of cost calculation.
    /// The caller supplies terminal evidence; this method does not touch DMA.
    pub fn completed<W>(self, work: W) -> AirtimeCompletion<'owner, K, W> {
        AirtimeCompletion {
            publication: self,
            work,
        }
    }
}

/// One terminal exchange and its still-unsettled budget, including failures.
/// Unknown cost must retain this owner, not silently refund or charge zero.
/// This value owns no packet or DMA buffer unless the caller puts one in `W`.
#[must_use = "settle terminal work or retain its unresolved charge"]
#[derive(Debug, Eq, PartialEq)]
pub struct AirtimeCompletion<'owner, K, W> {
    publication: AirtimeInFlight<'owner, K>,
    work: W,
}

impl<K: Copy, W> AirtimeCompletion<'_, K, W> {
    pub const fn key(&self) -> K {
        self.publication.key()
    }

    pub const fn reserved_micros(&self) -> NonZeroU32 {
        self.publication.reserved_micros()
    }

    pub const fn work(&self) -> &W {
        &self.work
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AirtimeError {
    AccountCapacity,
    ReservationCapacity,
    RetiredKey,
    UnknownReservation,
    WrongScheduler,
    ArithmeticOverflow,
    /// Eligible demand remained unfunded after checked round advancement.
    UnfundedDemand {
        cursor: usize,
    },
}

/// The funded place granted first from `cursor`: an account held there
/// whose balance reaches its least grant.
fn funded_place<const PEERS: usize>(
    minimums: &[u32; PEERS],
    balances: &[i64; PEERS],
    held: &[bool; PEERS],
    cursor: usize,
) -> Result<usize, AirtimeError> {
    minimums
        .iter()
        .zip(balances)
        .zip(held)
        .enumerate()
        .filter(|(_, ((minimum, balance), held))| {
            **held && **minimum != 0 && **balance >= i64::from(**minimum)
        })
        .map(|(index, _)| index)
        .min_by_key(|index| (*index < cursor, *index))
        .ok_or(AirtimeError::UnfundedDemand { cursor })
}

/// What one selection decided, place by place over the accounts, which it
/// reads in place: nothing is copied of them, and nothing changes until it
/// is committed.
struct Selection<K, const PEERS: usize> {
    /// Each place's largest least grant over the candidates; zero for none.
    minimums: [u32; PEERS],
    /// A candidate's key no account held, at the place its new account takes.
    fresh: [Option<K>; PEERS],
    /// Each held or new account's balance as the selection leaves it.
    balances: [i64; PEERS],
    /// The funded account it grants, by its place.
    selected: Option<usize>,
    /// The reservation place a grant takes.
    place: usize,
}

/// What [`AirtimeScheduler::reserve_for`] did.
#[must_use = "settle a published reservation or cancel unpublished work"]
#[derive(Debug, Eq, PartialEq)]
pub enum Reserved<'owner, K> {
    /// A reservation of a wanted key.
    This(AirtimeReservation<'owner, K>),
    /// No key is granted now, as [`AirtimeScheduler::reserve`] would say.
    Nothing,
    /// The grant would go to this key, which was not wanted: nothing changed.
    Other(K),
}

/// Stable CPU metadata for one airtime ledger. No packet or DMA storage.
/// Borrow `scheduler()` for selection and completion. Reservations borrow the
/// same identity, so storage cannot move, be replaced or be borrowed for another
/// scheduler while a reservation remains usable. Dropping a reservation does
/// not refund it; borrowing the storage again preserves outstanding accounts.
///
/// A live reservation prevents replacement of its storage:
/// ```compile_fail,E0505
/// use core::num::NonZeroU32;
/// use oer_protocols_ieee80211_core_upper_mac::airtime::{AirtimeStorage, AirtimeCandidate};
/// let us = NonZeroU32::new(100).unwrap();
/// let mut storage = AirtimeStorage::<_, 1>::new(us);
/// let mut scheduler = storage.scheduler();
/// let reservation = scheduler.reserve([AirtimeCandidate { key: 1, minimum_micros: us }], &mut ())
///     .unwrap().unwrap();
/// drop(scheduler);
/// drop(storage);
/// let _ = reservation.key();
/// ```
pub struct AirtimeStorage<K, const PEERS: usize, const RESERVATIONS: usize = 2> {
    identity: u8,
    state: State<K, PEERS, RESERVATIONS>,
}

struct State<K, const PEERS: usize, const RESERVATIONS: usize> {
    accounts: [Option<Account<K>>; PEERS],
    /// Each reservation at its place.
    pending: [Option<Pending<K>>; RESERVATIONS],
    /// The places ended reservations freed, the last freed at `freed - 1`:
    /// the next reservation takes that one.
    free: [usize; RESERVATIONS],
    freed: usize,
    /// No reservation held a place from here up yet.
    unused: usize,
    quantum: NonZeroU32,
    cursor: usize,
    next_id: u64,
}

impl<K, const PEERS: usize, const RESERVATIONS: usize> State<K, PEERS, RESERVATIONS> {
    /// The place the next reservation takes, without taking it.
    fn vacant(&self) -> Option<usize> {
        match self.freed.checked_sub(1) {
            Some(top) => self.free.get(top).copied(),
            None => (self.unused < RESERVATIONS).then_some(self.unused),
        }
    }

    /// Take the place [`Self::vacant`] named.
    fn occupy(&mut self) {
        match self.freed.checked_sub(1) {
            Some(top) => self.freed = top,
            None => self.unused = self.unused.saturating_add(1),
        }
    }
}

impl<K: Copy + Eq, const PEERS: usize, const RESERVATIONS: usize>
    AirtimeStorage<K, PEERS, RESERVATIONS>
{
    /// Empty metadata with nonzero peer and reservation capacities.
    ///
    /// ```compile_fail,E0080
    /// use core::num::NonZeroU32;
    /// use oer_protocols_ieee80211_core_upper_mac::airtime::AirtimeStorage;
    /// let _ = AirtimeStorage::<u8, 0>::new(NonZeroU32::MIN);
    /// ```
    ///
    /// ```compile_fail,E0080
    /// use core::num::NonZeroU32;
    /// use oer_protocols_ieee80211_core_upper_mac::airtime::AirtimeStorage;
    /// let _ = AirtimeStorage::<u8, 1, 0>::new(NonZeroU32::MIN);
    /// ```
    pub const fn new(quantum_micros: NonZeroU32) -> Self {
        const {
            if PEERS == 0 || RESERVATIONS == 0 {
                panic!("airtime storage needs peers and reservations");
            }
            // Each observation counts at most this many u32 budgets, so its
            // u32 count and u64 sum are representable on every target.
            if RESERVATIONS > u32::MAX as usize {
                panic!("airtime observation reservation count must fit u32");
            }
        }
        Self {
            identity: 0,
            state: State {
                accounts: [None; PEERS],
                pending: [None; RESERVATIONS],
                free: [0; RESERVATIONS],
                freed: 0,
                unused: 0,
                quantum: quantum_micros,
                cursor: 0,
                next_id: 0,
            },
        }
    }

    pub fn scheduler(&mut self) -> AirtimeScheduler<'_, K, PEERS, RESERVATIONS> {
        AirtimeScheduler {
            origin: Origin(&self.identity),
            state: &mut self.state,
        }
    }
}

/// Equal-quantum deficit scheduler. A grant is deducted immediately, so a
/// prepared successor cannot reuse the active exchange's outstanding budget.
/// Settlement replaces the reservation with the supplied modelled charge,
/// including retries. Deficits represent service, never measured CPU residence.
///
/// A caller supplies durable eligible demand on each selection. Empty/asleep
/// peers accrue no new credit and lose positive surplus; existing debt stays
/// until repaid or the radio explicitly retires that association. Metadata is
/// never evicted merely because a queue is empty. Exhausted rounds advance
/// arithmetically in one step, without a retry loop, timer or synthetic wake.
pub struct AirtimeScheduler<'owner, K, const PEERS: usize, const RESERVATIONS: usize = 2> {
    origin: Origin<'owner>,
    state: &'owner mut State<K, PEERS, RESERVATIONS>,
}

impl<'owner, K: Copy + Eq, const PEERS: usize, const RESERVATIONS: usize>
    AirtimeScheduler<'owner, K, PEERS, RESERVATIONS>
{
    /// Select once per outer key; packet queues are inspected but untouched.
    /// Errors are transactional, including capacity/overflow failures. A full
    /// reservation horizon requires a completion/cancellation edge, not polling.
    /// Each successful transition is reported to `events` as an
    /// [`AirtimeObservation`].
    pub fn reserve(
        &mut self,
        candidates: impl IntoIterator<Item = AirtimeCandidate<K>>,
        events: &mut impl EventSink<AirtimeObservation<K>>,
    ) -> Result<Option<AirtimeReservation<'owner, K>>, AirtimeError> {
        let selection = self.select(candidates)?;
        self.commit(selection, events)
    }

    /// The key [`Self::reserve`] would grant now over `candidates`, changing
    /// nothing; `None` when it would grant none.
    ///
    /// # Errors
    /// What [`Self::reserve`] would return.
    pub fn selects(
        &self,
        candidates: impl IntoIterator<Item = AirtimeCandidate<K>>,
    ) -> Result<Option<K>, AirtimeError> {
        let selection = self.select(candidates)?;
        Ok(self.key_of(&selection))
    }

    /// [`Self::reserve`] when it would grant a key `wanted` takes, or grant
    /// none; when it would grant another key, nothing changes and that key
    /// comes back ([`Reserved::Other`]).
    ///
    /// # Errors
    /// What [`Self::reserve`] would return.
    pub fn reserve_for(
        &mut self,
        wanted: impl FnOnce(K) -> bool,
        candidates: impl IntoIterator<Item = AirtimeCandidate<K>>,
        events: &mut impl EventSink<AirtimeObservation<K>>,
    ) -> Result<Reserved<'owner, K>, AirtimeError> {
        let selection = self.select(candidates)?;
        if let Some(key) = self.key_of(&selection)
            && !wanted(key)
        {
            return Ok(Reserved::Other(key));
        }
        Ok(match self.commit(selection, events)? {
            Some(reservation) => Reserved::This(reservation),
            None => Reserved::Nothing,
        })
    }

    /// What [`Self::reserve`] decides over `candidates`, reading the
    /// accounts in place: what each account would hold, and which is
    /// granted. Nothing changes until [`Self::commit`].
    fn select(
        &self,
        candidates: impl IntoIterator<Item = AirtimeCandidate<K>>,
    ) -> Result<Selection<K, PEERS>, AirtimeError> {
        let place = self
            .state
            .vacant()
            .ok_or(AirtimeError::ReservationCapacity)?;
        let accounts = &self.state.accounts;
        let mut minimums = [0_u32; PEERS];
        let mut fresh: [Option<K>; PEERS] = [None; PEERS];
        for candidate in candidates {
            let held = accounts
                .iter()
                .position(|slot| slot.as_ref().is_some_and(|a| a.key == candidate.key));
            let index = match held {
                Some(index) => {
                    if accounts
                        .get(index)
                        .and_then(Option::as_ref)
                        .is_some_and(|account| account.retired)
                    {
                        return Err(AirtimeError::RetiredKey);
                    }
                    index
                }
                None => match fresh.iter().position(|key| *key == Some(candidate.key)) {
                    Some(index) => index,
                    // A new account takes the first place no account holds.
                    None => {
                        let index = accounts
                            .iter()
                            .zip(&fresh)
                            .position(|(slot, key)| slot.is_none() && key.is_none())
                            .ok_or(AirtimeError::AccountCapacity)?;
                        if let Some(key) = fresh.get_mut(index) {
                            *key = Some(candidate.key);
                        }
                        index
                    }
                },
            };
            if let Some(minimum) = minimums.get_mut(index) {
                *minimum = (*minimum).max(candidate.minimum_micros.get());
            }
        }
        // One pass: each account's balance as the selection holds it (an
        // idle one keeps its debt and no surplus, a new one starts at zero),
        // whether one is funded already, and else the fewest rounds that
        // fund one.
        let quantum = i64::from(self.state.quantum.get());
        let mut balances = [0_i64; PEERS];
        let mut held = [false; PEERS];
        let mut funded = false;
        let mut rounds: Option<i64> = None;
        // An overflow counts only where the rounds are needed: when no
        // account is funded already.
        let mut overflow = false;
        let places = accounts
            .iter()
            .zip(&fresh)
            .zip(minimums)
            .zip(balances.iter_mut().zip(held.iter_mut()));
        for (((slot, key), minimum), (balance, live)) in places {
            *balance = match (slot, key) {
                (Some(account), _) if minimum == 0 => account.balance.min(0),
                (Some(account), _) => account.balance,
                (None, Some(_)) => 0,
                (None, None) => continue,
            };
            *live = true;
            if minimum == 0 || funded {
                continue;
            }
            let least = i64::from(minimum);
            if *balance >= least {
                funded = true;
                continue;
            }
            // The rounds that fund it: one when a quantum covers what it
            // lacks, as it does for every frame after a grant took its whole
            // balance; only a larger lack divides.
            let required = least.checked_sub(*balance).and_then(|needed| {
                if needed <= quantum {
                    Some(1)
                } else {
                    needed
                        .checked_sub(1)
                        .and_then(|needed| needed.checked_div(quantum))
                        .and_then(|rounds| rounds.checked_add(1))
                }
            });
            match required {
                Some(required) => {
                    rounds = Some(rounds.map_or(required, |old| old.min(required)));
                }
                None => overflow = true,
            }
        }
        if !funded {
            if overflow {
                return Err(AirtimeError::ArithmeticOverflow);
            }
            let Some(rounds) = rounds else {
                return Ok(Selection {
                    minimums,
                    fresh,
                    balances,
                    selected: None,
                    place,
                });
            };
            let credit = quantum
                .checked_mul(rounds)
                .ok_or(AirtimeError::ArithmeticOverflow)?;
            let places = balances.iter_mut().zip(held).zip(minimums);
            for ((balance, live), minimum) in places {
                if live && minimum != 0 {
                    *balance = balance
                        .checked_add(credit)
                        .ok_or(AirtimeError::ArithmeticOverflow)?;
                }
            }
        }
        let selected = funded_place(&minimums, &balances, &held, self.state.cursor)?;
        Ok(Selection {
            minimums,
            fresh,
            balances,
            selected: Some(selected),
            place,
        })
    }

    /// The key `selection` grants, if any.
    fn key_of(&self, selection: &Selection<K, PEERS>) -> Option<K> {
        let selected = selection.selected?;
        self.state
            .accounts
            .get(selected)
            .copied()
            .flatten()
            .map(|account| account.key)
            .or_else(|| selection.fresh.get(selected).copied().flatten())
    }

    /// Make `selection` the ledger's: its reservation, or, with none
    /// selected, its accounts as the selection left them. Every check comes
    /// before the first write, so an error changes nothing.
    fn commit(
        &mut self,
        selection: Selection<K, PEERS>,
        events: &mut impl EventSink<AirtimeObservation<K>>,
    ) -> Result<Option<AirtimeReservation<'owner, K>>, AirtimeError> {
        let Some(selected) = selection.selected else {
            self.apply(&selection);
            return Ok(None);
        };
        let cursor = self.state.cursor;
        let before = self.state.accounts.get(selected).copied().flatten();
        let key = before
            .map(|account| account.key)
            .or_else(|| selection.fresh.get(selected).copied().flatten())
            .ok_or(AirtimeError::UnfundedDemand { cursor })?;
        let balance = selection
            .balances
            .get(selected)
            .copied()
            .ok_or(AirtimeError::UnfundedDemand { cursor })?;
        let budget = u32::try_from(balance.min(i64::from(u32::MAX)))
            .ok()
            .and_then(NonZeroU32::new)
            .ok_or(AirtimeError::ArithmeticOverflow)?;
        let id = self
            .state
            .next_id
            .checked_add(1)
            .ok_or(AirtimeError::ArithmeticOverflow)?;
        let balance = balance
            .checked_sub(i64::from(budget.get()))
            .ok_or(AirtimeError::ArithmeticOverflow)?;
        // The cursor wraps at the bounded peer arena, rather than at usize::MAX.
        let next_cursor = selected
            .wrapping_add(1)
            .checked_rem(PEERS)
            .ok_or(AirtimeError::ArithmeticOverflow)?;
        if self.state.pending.get(selection.place).is_none() {
            return Err(AirtimeError::ReservationCapacity);
        }
        // At most `RESERVATIONS` reservations of u32 budgets, which the
        // storage's const bound keeps within both counts.
        let observed = Account {
            key,
            balance,
            minimum: selection.minimums.get(selected).copied().unwrap_or(0),
            retired: before.is_some_and(|account| account.retired),
            outstanding: before
                .map_or(0, |account| account.outstanding)
                .saturating_add(1),
            outstanding_micros: before
                .map_or(0, |account| account.outstanding_micros)
                .saturating_add(u64::from(budget.get())),
        };
        let reservation = AirtimeReservation {
            origin: self.origin,
            key,
            id,
            budget,
            place: selection.place,
        };
        self.apply(&selection);
        if let Some(slot) = self.state.accounts.get_mut(selected) {
            *slot = Some(observed);
        }
        if let Some(pending) = self.state.pending.get_mut(selection.place) {
            *pending = Some(Pending {
                id,
                key,
                budget: budget.get(),
            });
        }
        self.state.occupy();
        self.state.next_id = id;
        self.state.cursor = next_cursor;
        self.observe(observed, AirtimeAction::Granted, budget.get(), 0, events);
        Ok(Some(reservation))
    }

    /// Write what `selection` holds of each account: a new one made, a held
    /// one's least grant and balance.
    fn apply(&mut self, selection: &Selection<K, PEERS>) {
        let places = self
            .state
            .accounts
            .iter_mut()
            .zip(selection.fresh)
            .zip(selection.minimums)
            .zip(selection.balances);
        for (((slot, fresh), minimum), balance) in places {
            match (slot.as_mut(), fresh) {
                (Some(account), _) => {
                    account.minimum = minimum;
                    account.balance = balance;
                }
                (None, Some(key)) => {
                    *slot = Some(Account {
                        key,
                        balance,
                        minimum,
                        retired: false,
                        outstanding: 0,
                        outstanding_micros: 0,
                    });
                }
                (None, None) => {}
            }
        }
    }

    /// Complete all publications of this reservation at their modelled cost.
    /// Unknown/saturated work needs an explicit caller policy; it must not be
    /// converted to zero here. Errors return the receipt and still-outstanding reservation together.
    /// Successful settlement returns the receipt for reporting; it cannot be
    /// charged again without its consumed reservation.
    pub fn settle<W>(
        &mut self,
        completion: AirtimeCompletion<'owner, K, W>,
        charged_micros: NonZeroU32,
        events: &mut impl EventSink<AirtimeObservation<K>>,
    ) -> Result<W, (AirtimeError, AirtimeCompletion<'owner, K, W>)> {
        let AirtimeCompletion { publication, work } = completion;
        match self.finish(publication.reservation, charged_micros.get(), events) {
            Ok(()) => Ok(work),
            Err((error, reservation)) => Err((
                error,
                AirtimeCompletion {
                    publication: AirtimeInFlight { reservation },
                    work,
                },
            )),
        }
    }

    /// Refund work that was never published. A published timeout/abort belongs
    /// to settlement, because it still committed transmission work.
    pub fn cancel(
        &mut self,
        reservation: AirtimeReservation<'owner, K>,
        events: &mut impl EventSink<AirtimeObservation<K>>,
    ) -> Result<(), (AirtimeError, AirtimeReservation<'owner, K>)> {
        self.finish(reservation, 0, events)
    }

    fn finish(
        &mut self,
        reservation: AirtimeReservation<'owner, K>,
        charged: u32,
        events: &mut impl EventSink<AirtimeObservation<K>>,
    ) -> Result<(), (AirtimeError, AirtimeReservation<'owner, K>)> {
        if reservation.origin != self.origin {
            return Err((AirtimeError::WrongScheduler, reservation));
        }
        // Its own place, read directly: a later reservation there has
        // another id.
        let Some(slot) = self
            .state
            .pending
            .get_mut(reservation.place)
            .filter(|slot| {
                slot.as_ref().is_some_and(|p| {
                    p.id == reservation.id
                        && p.budget == reservation.budget.get()
                        && p.key == reservation.key
                })
            })
        else {
            return Err((AirtimeError::UnknownReservation, reservation));
        };
        // A held place is not among the freed ones, so they leave room for
        // it; the check keeps a broken count from losing the place.
        let Some(free) = self.state.free.get_mut(self.state.freed) else {
            return Err((AirtimeError::UnknownReservation, reservation));
        };
        let Some(account_slot) = self
            .state
            .accounts
            .iter_mut()
            .find(|a| a.as_ref().is_some_and(|a| a.key == reservation.key))
        else {
            return Err((AirtimeError::UnknownReservation, reservation));
        };
        let Some(account) = account_slot.as_mut() else {
            return Err((AirtimeError::UnknownReservation, reservation));
        };
        let Some(balance) = i64::from(reservation.budget.get())
            .checked_sub(i64::from(charged))
            .and_then(|refund| account.balance.checked_add(refund))
        else {
            return Err((AirtimeError::ArithmeticOverflow, reservation));
        };
        account.balance = if account.minimum == 0 {
            balance.min(0)
        } else {
            balance
        };
        account.outstanding = account.outstanding.saturating_sub(1);
        account.outstanding_micros = account
            .outstanding_micros
            .saturating_sub(u64::from(reservation.budget.get()));
        let observed = *account;
        *slot = None;
        *free = reservation.place;
        self.state.freed = self.state.freed.saturating_add(1);
        if account.retired && account.outstanding == 0 {
            *account_slot = None;
        }
        self.observe(
            observed,
            if charged == 0 {
                AirtimeAction::Cancelled
            } else {
                AirtimeAction::Settled
            },
            reservation.budget.get(),
            charged,
            events,
        );
        Ok(())
    }

    /// End one radio association. Outstanding completions keep their old
    /// account pinned and cannot debit a newly associated generation's key.
    pub fn retire(&mut self, key: K) {
        self.retire_where(|candidate| candidate == key);
    }

    /// Retire matching generations; pending tokens pin their old accounts.
    pub fn retire_where(&mut self, mut retired: impl FnMut(K) -> bool) {
        for slot in &mut self.state.accounts {
            if let Some(account) = slot.as_mut()
                && retired(account.key)
            {
                if account.outstanding != 0 {
                    account.retired = true;
                    account.minimum = 0;
                } else {
                    *slot = None;
                }
            }
        }
    }

    /// Modelled remaining credit (negative is debt), including reservations.
    pub fn balance_micros(&self, key: K) -> Option<i64> {
        self.state
            .accounts
            .iter()
            .flatten()
            .find(|a| a.key == key)
            .map(|a| a.balance)
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    clippy::arithmetic_side_effects
)]
mod tests;
