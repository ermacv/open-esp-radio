//! Bounded deficit scheduling of radio peers, independent of packet storage.
//!
//! Reserve before preparing hardware work, settle after terminal completion,
//! and cancel only unpublished work. Active and standby batches share this
//! account. Keys must include the radio association generation, but not a
//! transport flow identity. All methods are synchronous and allocation-free.

use core::num::NonZeroU32;

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
}

#[derive(Clone, Copy)]
struct Pending {
    id: u64,
    account: usize,
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
/// use oer_ieee80211_datapath::airtime::{AirtimeStorage, AirtimeCandidate};
/// let us = NonZeroU32::new(100).unwrap();
/// let mut storage = AirtimeStorage::<_, 1>::new(us);
/// let mut scheduler = storage.scheduler();
/// let reservation = scheduler.reserve([AirtimeCandidate { key: 1, minimum_micros: us }])
///     .unwrap().unwrap();
/// scheduler.cancel(reservation.published());
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
/// use oer_ieee80211_datapath::airtime::{AirtimeStorage, AirtimeCandidate};
/// let us = NonZeroU32::new(100).unwrap();
/// let mut storage = AirtimeStorage::<_, 1>::new(us);
/// let mut scheduler = storage.scheduler();
/// let reservation = scheduler.reserve([AirtimeCandidate { key: 1, minimum_micros: us }])
///     .unwrap().unwrap();
/// drop(scheduler);
/// drop(storage);
/// let _ = reservation.key();
/// ```
pub struct AirtimeStorage<K, const PEERS: usize, const RESERVATIONS: usize = 2> {
    identity: u8,
    state: State<K, PEERS, RESERVATIONS>,
    observer: Option<fn(AirtimeObservation<K>)>,
}

struct State<K, const PEERS: usize, const RESERVATIONS: usize> {
    accounts: [Option<Account<K>>; PEERS],
    pending: [Option<Pending>; RESERVATIONS],
    quantum: NonZeroU32,
    cursor: usize,
    next_id: u64,
}

impl<K: Copy + Eq, const PEERS: usize, const RESERVATIONS: usize>
    AirtimeStorage<K, PEERS, RESERVATIONS>
{
    pub const fn new(quantum_micros: NonZeroU32) -> Self {
        assert!(PEERS > 0 && RESERVATIONS > 0);
        Self {
            identity: 0,
            observer: None,
            state: State {
                accounts: [None; PEERS],
                pending: [None; RESERVATIONS],
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
            observer: self.observer,
        }
    }

    /// Observe successful ledger transitions. The synchronous callback must
    /// remain bounded and must not log, wait, or re-enter the scheduler.
    pub const fn with_observer(mut self, observer: Option<fn(AirtimeObservation<K>)>) -> Self {
        self.observer = observer;
        self
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
    observer: Option<fn(AirtimeObservation<K>)>,
}

impl<'owner, K: Copy + Eq, const PEERS: usize, const RESERVATIONS: usize>
    AirtimeScheduler<'owner, K, PEERS, RESERVATIONS>
{
    /// Select once per outer key; packet queues are inspected but untouched.
    /// Errors are transactional, including capacity/overflow failures. A full
    /// reservation horizon requires a completion/cancellation edge, not polling.
    pub fn reserve(
        &mut self,
        candidates: impl IntoIterator<Item = AirtimeCandidate<K>>,
    ) -> Result<Option<AirtimeReservation<'owner, K>>, AirtimeError> {
        let pending_index = self
            .state
            .pending
            .iter()
            .position(Option::is_none)
            .ok_or(AirtimeError::ReservationCapacity)?;
        // Keep candidate admission atomic. This bounded value-only copy owns
        // no packets, does not allocate, and never crosses an await boundary.
        let mut accounts = self.state.accounts;
        for account in accounts.iter_mut().flatten() {
            account.minimum = 0;
        }
        for candidate in candidates {
            let index = accounts
                .iter()
                .position(|a| a.is_some_and(|a| a.key == candidate.key))
                .or_else(|| accounts.iter().position(Option::is_none))
                .ok_or(AirtimeError::AccountCapacity)?;
            let account = accounts[index].get_or_insert(Account {
                key: candidate.key,
                balance: 0,
                minimum: 0,
                retired: false,
            });
            if account.retired {
                return Err(AirtimeError::RetiredKey);
            }
            account.minimum = account.minimum.max(candidate.minimum_micros.get());
        }
        for account in accounts.iter_mut().flatten() {
            if account.minimum == 0 {
                account.balance = account.balance.min(0);
            }
        }
        let choose = |accounts: &[Option<Account<K>>; PEERS]| {
            (0..PEERS)
                .map(|offset| (self.state.cursor + offset) % PEERS)
                .find(|&i| {
                    accounts[i].is_some_and(|a| a.minimum != 0 && a.balance >= i64::from(a.minimum))
                })
        };
        let selected = if let Some(index) = choose(&accounts) {
            index
        } else {
            let quantum = i64::from(self.state.quantum.get());
            let mut rounds: Option<i64> = None;
            for account in accounts.iter().flatten().filter(|a| a.minimum != 0) {
                let needed = i64::from(account.minimum)
                    .checked_sub(account.balance)
                    .ok_or(AirtimeError::ArithmeticOverflow)?;
                let required = (needed - 1) / quantum + 1;
                rounds = Some(rounds.map_or(required, |old| old.min(required)));
            }
            let Some(rounds) = rounds else {
                self.state.accounts = accounts;
                return Ok(None);
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
            choose(&accounts).expect("one eligible minimum was funded")
        };
        let account = accounts[selected].as_mut().expect("selected account");
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
        };
        account.balance -= i64::from(budget.get());
        self.state.accounts = accounts;
        self.state.pending[pending_index] = Some(Pending {
            id,
            account: selected,
            budget: budget.get(),
        });
        self.state.next_id = id;
        self.state.cursor = (selected + 1) % PEERS;
        self.observe(selected, AirtimeAction::Granted, budget.get(), 0);
        Ok(Some(reservation))
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
    ) -> Result<W, (AirtimeError, AirtimeCompletion<'owner, K, W>)> {
        let AirtimeCompletion { publication, work } = completion;
        match self.finish(publication.reservation, charged_micros.get()) {
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
    ) -> Result<(), (AirtimeError, AirtimeReservation<'owner, K>)> {
        self.finish(reservation, 0)
    }

    fn finish(
        &mut self,
        reservation: AirtimeReservation<'owner, K>,
        charged: u32,
    ) -> Result<(), (AirtimeError, AirtimeReservation<'owner, K>)> {
        if reservation.origin != self.origin {
            return Err((AirtimeError::WrongScheduler, reservation));
        }
        let Some(index) = self.state.pending.iter().position(|p| {
            p.is_some_and(|p| {
                p.id == reservation.id
                    && p.budget == reservation.budget.get()
                    && self.state.accounts[p.account].is_some_and(|a| a.key == reservation.key)
            })
        }) else {
            return Err((AirtimeError::UnknownReservation, reservation));
        };
        let pending = self.state.pending[index].expect("checked pending reservation");
        let account = self.state.accounts[pending.account]
            .as_mut()
            .expect("pending pins account");
        let Some(balance) = account
            .balance
            .checked_add(i64::from(pending.budget) - i64::from(charged))
        else {
            return Err((AirtimeError::ArithmeticOverflow, reservation));
        };
        account.balance = if account.minimum == 0 {
            balance.min(0)
        } else {
            balance
        };
        self.state.pending[index] = None;
        self.observe(
            pending.account,
            if charged == 0 {
                AirtimeAction::Cancelled
            } else {
                AirtimeAction::Settled
            },
            pending.budget,
            charged,
        );
        let account = self.state.accounts[pending.account]
            .as_ref()
            .expect("pending pins account");
        if account.retired
            && !self
                .state
                .pending
                .iter()
                .flatten()
                .any(|p| p.account == pending.account)
        {
            self.state.accounts[pending.account] = None;
        }
        Ok(())
    }

    /// End one radio association. Outstanding completions keep their old
    /// account pinned and cannot debit a newly associated generation's key.
    pub fn retire(&mut self, key: K) {
        let Some(index) = self
            .state
            .accounts
            .iter()
            .position(|a| a.is_some_and(|a| a.key == key))
        else {
            return;
        };
        if self
            .state
            .pending
            .iter()
            .flatten()
            .any(|p| p.account == index)
        {
            let account = self.state.accounts[index]
                .as_mut()
                .expect("checked account");
            account.retired = true;
            account.minimum = 0;
        } else {
            self.state.accounts[index] = None;
        }
    }

    /// Retire matching generations without exposing account storage. Pending
    /// tokens keep their old accounts pinned until cancellation or settlement.
    pub fn retire_where(&mut self, mut retired: impl FnMut(K) -> bool) {
        for index in 0..PEERS {
            if let Some(key) = self.state.accounts[index].map(|account| account.key)
                && retired(key)
            {
                self.retire(key);
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
mod tests;
