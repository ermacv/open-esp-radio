//! The airtime policy of a network's egress over one radio.
//!
//! Peers share the air by [`airtime`](oer_protocols_ieee80211_core_upper_mac::airtime)
//! deficits, keyed by peer and never by flow, so a peer's flows take turns within
//! its share. What the radio knows of a peer stays behind [`PeerAir`], and what
//! the air charged or which association ended comes back in a [`RadioNotice`]:
//! the network sees neither.

use core::num::NonZeroU32;

/// The full rate a settled frame's report names ([`PeerAir::reported`]).
pub use oer_protocols_ieee80211_contracts_port::PhyRate;
use oer_protocols_ieee80211_core_upper_mac::airtime::{
    AirtimeCandidate, AirtimeError, AirtimeInFlight, AirtimeReservation, AirtimeScheduler,
    AirtimeStorage, Reserved,
};
use oer_protocols_network_port::{Candidate, Egress, Next, PlanStop, QUANTUM, Ready, Turns};

/// What the radio knows of the peers.
pub trait PeerAir {
    /// An association: a peer with its generation, never a flow.
    type Key: Copy + Eq;
    /// What [`Self::candidate`] and [`Self::ceiling`] read besides the work:
    /// their answers stand while it is the same.
    type View: Copy + Eq;

    /// The view now.
    fn view(&self) -> Self::View;

    /// The account `ready` spends and its least useful grant, or `None` while
    /// this work may not go: its peer sleeps or has left.
    fn candidate(&self, ready: &Ready) -> Option<AirtimeCandidate<Self::Key>>;

    /// The most `ready` can cost, every retry at the lowest rate included. A
    /// frame whose cost is not known is charged this, so no peer gains by not
    /// knowing it. It holds only as far as this is a true ceiling.
    fn ceiling(&self, ready: &Ready) -> NonZeroU32;

    /// A frame of `key`'s association settled, its last transmission on the
    /// air at the full rate `rate`.
    fn reported(&mut self, key: Self::Key, rate: PhyRate);
}

/// How a frame ended, as a radio port's completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Exchange<T> {
    pub ticket: T,
    /// What the air cost, or `None` when that is not known, which charges the
    /// frame's ceiling or its reservation, whichever is more.
    pub charged: Option<NonZeroU32>,
    /// The full rate of the frame's last transmission with an established
    /// rate; `None` when none has one, which is no proof that nothing was
    /// sent.
    pub rate: Option<PhyRate>,
}

/// What a radio port tells the policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioNotice<T, K> {
    /// A frame the port took has ended.
    Ended(Exchange<T>),
    /// An association has ended. Each of its frames still at the port
    /// settles here, once, at its ceiling or its reservation if that is
    /// more, and its account goes at once. The frames' buffers stay the
    /// port's until they come back; each one's later [`Self::Ended`] only
    /// closes its ticket, with no second charge.
    Left(K),
    /// The port took the frame and lost it with its memory before the
    /// radio's end of it: no completion follows, and whether it reached the
    /// air is not known. Its reservation settles here at its ceiling, or its
    /// reservation if that is more. This settles the account only: it
    /// returns no memory and permits no reuse of the lost buffer.
    Lost(T),
}

/// Why the policy cannot go on. The scheduler keeps a charge it could not
/// settle, so nothing is refunded by a fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AirtimeFault {
    Airtime(AirtimeError),
    /// An accepted frame's ticket names no place below the reservations: the
    /// port has more buffers than the policy reserves for. The frame settled
    /// at once at its ceiling, or its reservation if that is more, as a frame
    /// of unknown end does.
    NoPlace,
    /// An accepted frame's ticket names a frame still in flight: the port
    /// gave its buffer again before that frame's notice. The frame in flight
    /// keeps its place; the new one settled at once as for [`Self::NoPlace`].
    TicketInUse,
    /// A completion names no frame in flight.
    UnknownTicket,
}

impl From<AirtimeError> for AirtimeFault {
    fn from(error: AirtimeError) -> Self {
        Self::Airtime(error)
    }
}

/// What a plan keeps of one account of the ready work.
#[derive(Clone, Copy, Debug)]
struct Account<K> {
    key: K,
    /// The largest least grant among its works.
    minimum: NonZeroU32,
    /// Its works whose least grant is that largest.
    holders: usize,
    /// Its works.
    works: usize,
}

impl<K: Copy> Account<K> {
    fn new(candidate: AirtimeCandidate<K>) -> Self {
        Self {
            key: candidate.key,
            minimum: candidate.minimum_micros,
            holders: 1,
            works: 1,
        }
    }

    fn count(&mut self, least: NonZeroU32) {
        self.works = self.works.saturating_add(1);
        match least.cmp(&self.minimum) {
            core::cmp::Ordering::Greater => {
                self.minimum = least;
                self.holders = 1;
            }
            core::cmp::Ordering::Equal => self.holders = self.holders.saturating_add(1),
            core::cmp::Ordering::Less => {}
        }
    }

    /// One of its works went, whose least grant was `least`.
    fn sent(&mut self, least: NonZeroU32) {
        self.works = self.works.saturating_sub(1);
        if least == self.minimum {
            self.holders = self.holders.saturating_sub(1);
        }
    }

    fn candidate(&self) -> Option<AirtimeCandidate<K>> {
        (self.works != 0).then_some(AirtimeCandidate {
            key: self.key,
            minimum_micros: self.minimum,
        })
    }
}

/// One collection of the ready work: each account's candidate, as a choice
/// gives the scheduler, and the turns of the account the scheduler serves
/// first, which the next frames take while that holds.
pub struct Plan<C, V, K, const PEERS: usize> {
    /// The air's view when the plan was collected.
    view: V,
    accounts: heapless::Vec<Account<K>, PEERS>,
    /// An account past `PEERS`: with it the scheduler cannot hold them all,
    /// and says so as it would over the whole list.
    extra: Option<Account<K>>,
    /// The account whose turns the plan holds.
    account: Option<K>,
    /// Its works, with their least grants.
    turns: Turns<(C, NonZeroU32), QUANTUM>,
    /// Its last served flow, once the plan's frames before are accepted.
    last: Option<u8>,
}

impl<C, V, K: Copy + Eq, const PEERS: usize> Plan<C, V, K, PEERS> {
    fn count(&mut self, candidate: AirtimeCandidate<K>) {
        let least = candidate.minimum_micros;
        if let Some(account) = self
            .accounts
            .iter_mut()
            .chain(&mut self.extra)
            .find(|account| account.key == candidate.key)
        {
            account.count(least);
            return;
        }
        if let Err(account) = self.accounts.push(Account::new(candidate))
            && self.extra.is_none()
        {
            self.extra = Some(account);
        }
    }

    fn candidates(&self) -> impl Iterator<Item = AirtimeCandidate<K>> + '_ {
        self.accounts
            .iter()
            .chain(&self.extra)
            .filter_map(Account::candidate)
    }

    fn served(&mut self) -> Option<&mut Account<K>> {
        let key = self.account?;
        self.accounts.iter_mut().find(|account| account.key == key)
    }
}

/// For a test: a work and the reservation it was chosen under.
#[cfg(test)]
pub(crate) type Chosen<'o, C, K> = (C, Held<'o, K>);

/// A choice until the port answers it.
#[derive(Debug)]
pub struct Held<'o, K> {
    reservation: AirtimeReservation<'o, K>,
    flow: Option<u8>,
    ceiling: NonZeroU32,
}

/// A frame the port holds, at its ticket's place.
enum Flight<'o, K> {
    /// Its reservation is open, for its completion to settle.
    Open {
        flight: AirtimeInFlight<'o, K>,
        ceiling: NonZeroU32,
    },
    /// Its association left and its reservation settled at its ceiling: only
    /// its buffer's return is owed, which closes the ticket and charges
    /// nothing.
    Closed,
}

/// Airtime between peers, turns between one peer's flows, and control first.
///
/// A ticket `T` names its frame's buffer: its place, below `RESERVATIONS`,
/// which no other frame of the port has until that frame's notice. Each
/// frame in flight is kept at its ticket's place, so accepting a frame and
/// hearing of its end read that place alone.
pub struct AirtimeEgress<'o, A: PeerAir, T, const PEERS: usize, const RESERVATIONS: usize> {
    scheduler: AirtimeScheduler<'o, A::Key, PEERS, RESERVATIONS>,
    air: A,
    flights: [Option<Flight<'o, A::Key>>; RESERVATIONS],
    /// Each peer's last served flow.
    flows: [Option<(A::Key, u8)>; PEERS],
    ticket: core::marker::PhantomData<fn(T)>,
}

impl<'o, A: PeerAir, T, const PEERS: usize, const RESERVATIONS: usize>
    AirtimeEgress<'o, A, T, PEERS, RESERVATIONS>
{
    pub fn new(storage: &'o mut AirtimeStorage<A::Key, PEERS, RESERVATIONS>, air: A) -> Self {
        Self {
            scheduler: storage.scheduler(),
            air,
            flights: [const { None }; RESERVATIONS],
            flows: [const { None }; PEERS],
            ticket: core::marker::PhantomData,
        }
    }

    pub const fn air(&self) -> &A {
        &self.air
    }

    pub const fn air_mut(&mut self) -> &mut A {
        &mut self.air
    }

    /// A peer's modelled credit; negative is debt.
    pub fn balance_micros(&self, key: A::Key) -> Option<i64> {
        self.scheduler.balance_micros(key)
    }

    /// Frames the port still holds, settled or not: one per buffer it has
    /// not given back.
    pub fn in_flight(&self) -> usize {
        self.flights.iter().flatten().count()
    }

    /// Frames whose reservation is still open.
    pub fn unsettled(&self) -> usize {
        self.flights
            .iter()
            .flatten()
            .filter(|flight| matches!(flight, Flight::Open { .. }))
            .count()
    }

    /// End an association: each of its frames still at the port settles now
    /// at its ceiling, or its reservation if that is more, and the account
    /// goes. The frames' tickets stay until their buffers come back.
    ///
    /// # Errors
    /// [`AirtimeFault`]: a settlement the ledger refused.
    pub fn retire(&mut self, key: A::Key) -> Result<(), AirtimeFault> {
        for slot in &mut self.flights {
            let closes = matches!(slot, Some(Flight::Open { flight, .. }) if flight.key() == key);
            if !closes {
                continue;
            }
            let Some(Flight::Open { flight, ceiling }) = slot.take() else {
                continue;
            };
            *slot = Some(Flight::Closed);
            let charged = ceiling.max(flight.reserved_micros());
            self.scheduler
                .settle(flight.completed(()), charged, &mut ())
                .map_err(|(error, _)| AirtimeFault::from(error))?;
        }
        self.scheduler.retire(key);
        for slot in &mut self.flows {
            if slot.is_some_and(|(peer, _)| peer == key) {
                *slot = None;
            }
        }
        Ok(())
    }

    fn last_flow(&self, key: A::Key) -> Option<u8> {
        self.flows
            .iter()
            .flatten()
            .find(|(peer, _)| *peer == key)
            .map(|(_, flow)| *flow)
    }

    fn remember(&mut self, key: A::Key, flow: u8) {
        let place = self
            .flows
            .iter()
            .position(|slot| slot.is_some_and(|(peer, _)| peer == key))
            .or_else(|| self.flows.iter().position(Option::is_none))
            .and_then(|place| self.flows.get_mut(place));
        if let Some(slot) = place {
            *slot = Some((key, flow));
        }
    }

    /// For a test: the first frame of a fresh plan over `ready`, as one
    /// choice.
    #[cfg(test)]
    #[allow(clippy::panic, reason = "a fresh plan that stops fails the test")]
    pub(crate) fn first_choice<C: Candidate>(
        &mut self,
        ready: impl Iterator<Item = C> + Clone,
    ) -> Result<Option<Chosen<'o, C, A::Key>>, AirtimeFault>
    where
        T: Copy + Into<usize>,
    {
        let mut plan = <Self as Egress<T, RadioNotice<T, A::Key>>>::plan(self, ready)?;
        match <Self as Egress<T, RadioNotice<T, A::Key>>>::next(self, &mut plan)? {
            Next::Send(work, held) => Ok(Some((work, held))),
            Next::Idle => Ok(None),
            Next::Stop(stop) => panic!("a fresh plan stopped: {stop:?}"),
        }
    }

    /// `work` under `reservation`, with its ceiling.
    fn hold<C: Candidate>(
        &self,
        work: C,
        reservation: AirtimeReservation<'o, A::Key>,
    ) -> (C, Held<'o, A::Key>) {
        let flow = work.ready().flow;
        let ceiling = self.air.ceiling(work.ready());
        (
            work,
            Held {
                reservation,
                flow,
                ceiling,
            },
        )
    }

    /// Settle `flight` at `charged`.
    fn settle(
        &mut self,
        flight: AirtimeInFlight<'o, A::Key>,
        charged: NonZeroU32,
    ) -> Result<(), AirtimeFault> {
        self.scheduler
            .settle(flight.completed(()), charged, &mut ())
            .map_err(|(error, _)| error.into())
            .map(|()| ())
    }
}

impl<'o, A: PeerAir, T: Copy + Into<usize>, const PEERS: usize, const RESERVATIONS: usize>
    Egress<T, RadioNotice<T, A::Key>> for AirtimeEgress<'o, A, T, PEERS, RESERVATIONS>
{
    type Reservation = Held<'o, A::Key>;
    type Fault = AirtimeFault;

    type Plan<C: Candidate> = Plan<C, A::View, A::Key, PEERS>;

    /// One pass over `ready`: each work's candidate once, counted to its
    /// account as its largest least grant, and the first account's works in
    /// their turns. When the scheduler would serve another account first, a
    /// second pass takes that one's turns. More accounts than `PEERS` cannot
    /// all be served: the plan keeps one more, for the scheduler to say so.
    fn plan<C: Candidate>(
        &mut self,
        ready: impl Iterator<Item = C> + Clone,
    ) -> Result<Self::Plan<C>, AirtimeFault> {
        let mut plan = Plan {
            view: self.air.view(),
            accounts: heapless::Vec::new(),
            extra: None,
            account: None,
            turns: Turns::after(None),
            last: None,
        };
        for work in ready.clone() {
            let Some(candidate) = self.air.candidate(work.ready()) else {
                continue;
            };
            plan.count(candidate);
            let account = match plan.account {
                Some(account) => account,
                None => {
                    plan.account = Some(candidate.key);
                    plan.turns = Turns::after(self.last_flow(candidate.key));
                    candidate.key
                }
            };
            if account == candidate.key {
                let described = *work.ready();
                plan.turns
                    .offer(&described, (work, candidate.minimum_micros));
            }
        }
        // One account is the only one a grant can go to. Otherwise a look
        // at the scheduler names the first, or what it refuses, which the
        // plan's first step meets again.
        if plan.accounts.len() > 1
            && let Ok(Some(first)) = self.scheduler.selects(plan.candidates())
            && plan.account != Some(first)
        {
            plan.account = Some(first);
            plan.turns = Turns::after(self.last_flow(first));
            for work in ready {
                if let Some(candidate) = self.air.candidate(work.ready())
                    && candidate.key == first
                {
                    let described = *work.ready();
                    plan.turns
                        .offer(&described, (work, candidate.minimum_micros));
                }
            }
        }
        plan.last = plan.account.and_then(|account| self.last_flow(account));
        Ok(plan)
    }

    /// The next frame of the plan's account, reserved as a choice over the
    /// same ready work would reserve it: the scheduler is asked over every
    /// account's candidate, and grants only if it serves the plan's account.
    /// The plan stops, reserving nothing, when the air's view moved, when
    /// the account's last served flow is not the plan's last frame's, when
    /// its largest least grant went and the plan does not hold the next,
    /// when its turns are spent with more of its work ready, or when the
    /// scheduler would serve another account.
    fn next<C: Candidate>(
        &mut self,
        plan: &mut Self::Plan<C>,
    ) -> Result<Next<C, Self::Reservation>, AirtimeFault> {
        if self.air.view() != plan.view {
            return Ok(Next::Stop(PlanStop::Changed));
        }
        let account = plan.account;
        if let Some(key) = account
            && self.last_flow(key) != plan.last
        {
            return Ok(Next::Stop(PlanStop::Changed));
        }
        let spent = plan.turns.first().is_none();
        if let Some(served) = plan.served() {
            if served.works != 0 && served.holders == 0 {
                return Ok(Next::Stop(PlanStop::Grant));
            }
            if served.works != 0 && spent {
                return Ok(Next::Stop(PlanStop::Spent));
            }
        }
        let reserved =
            self.scheduler
                .reserve_for(|key| Some(key) == account, plan.candidates(), &mut ());
        let reservation = match reserved {
            Ok(Reserved::This(reservation)) => reservation,
            // A frame's completion makes room, and it wakes the service.
            Ok(Reserved::Nothing) | Err(AirtimeError::ReservationCapacity) => {
                return Ok(Next::Idle);
            }
            Ok(Reserved::Other(_)) => return Ok(Next::Stop(PlanStop::Account)),
            Err(error) => return Err(error.into()),
        };
        // The scheduler granted the plan's account, a candidate only while
        // it has works, which its turns hold until they are spent.
        let Some((work, least)) = plan.turns.take() else {
            return match self.scheduler.cancel(reservation, &mut ()) {
                Ok(()) => Ok(Next::Stop(PlanStop::Spent)),
                Err((error, _)) => Err(error.into()),
            };
        };
        if let Some(served) = plan.served() {
            served.sent(least);
        }
        if let Some(flow) = work.ready().flow {
            plan.last = Some(flow);
        }
        let (work, held) = self.hold(work, reservation);
        Ok(Next::Send(work, held))
    }

    /// The step of a plan over no work: it names no account, so the scheduler
    /// is asked as [`Self::next`] asks it, over no candidate and for none.
    /// It grants none and changes what a selection over none changes: an
    /// idle account's surplus, never its debt, and with the reservation
    /// horizon full nothing at all. The air's view is not read: a plan's
    /// first step reads the view it was just collected with.
    fn nothing_ready(&mut self) -> Result<(), AirtimeFault> {
        match self
            .scheduler
            .reserve_for(|_| false, core::iter::empty(), &mut ())
        {
            // With the horizon full the scheduler refuses before any account
            // changes, as a plan's step meets it.
            Ok(Reserved::Nothing) | Err(AirtimeError::ReservationCapacity) => Ok(()),
            // A selection over no candidate names no account, so neither
            // comes; another account's turn changed nothing, and a grant goes
            // back, as a plan's step gives back one its turns cannot use.
            Ok(Reserved::Other(_)) => Ok(()),
            Ok(Reserved::This(reservation)) => self
                .scheduler
                .cancel(reservation, &mut ())
                .map_err(|(error, _)| error.into()),
            Err(error) => Err(error.into()),
        }
    }

    fn accepted(&mut self, held: Held<'o, A::Key>, ticket: T) -> Result<(), AirtimeFault> {
        let Held {
            reservation,
            flow,
            ceiling,
        } = held;
        if let Some(flow) = flow {
            self.remember(reservation.key(), flow);
        }
        // The port took the frame: from here its reservation is published
        // and only a settlement ends it, never a cancellation.
        let flight = reservation.published();
        let fault = match self.flights.get_mut(ticket.into()) {
            Some(slot @ None) => {
                *slot = Some(Flight::Open { flight, ceiling });
                return Ok(());
            }
            Some(Some(_)) => AirtimeFault::TicketInUse,
            None => AirtimeFault::NoPlace,
        };
        // No place keeps the frame for its end: it settles now as a frame of
        // unknown end, and the fault names why.
        let charged = ceiling.max(flight.reserved_micros());
        self.settle(flight, charged)?;
        Err(fault)
    }

    fn refused(&mut self, held: Held<'o, A::Key>) -> Result<(), AirtimeFault> {
        self.scheduler
            .cancel(held.reservation, &mut ())
            .map_err(|(error, _)| error.into())
    }

    fn unknown(&mut self, held: Held<'o, A::Key>) -> Result<(), AirtimeFault> {
        let flight = held.reservation.published();
        let charged = held.ceiling.max(flight.reserved_micros());
        self.settle(flight, charged)
    }

    fn notice(&mut self, notice: RadioNotice<T, A::Key>) -> Result<(), AirtimeFault> {
        let exchange = match notice {
            RadioNotice::Ended(exchange) => exchange,
            RadioNotice::Lost(ticket) => Exchange {
                ticket,
                charged: None,
                rate: None,
            },
            RadioNotice::Left(key) => return self.retire(key),
        };
        let flight = self
            .flights
            .get_mut(exchange.ticket.into())
            .and_then(Option::take);
        match flight {
            Some(Flight::Open { flight, ceiling }) => {
                let charged = exchange
                    .charged
                    .unwrap_or_else(|| ceiling.max(flight.reserved_micros()));
                let key = flight.key();
                self.settle(flight, charged)?;
                // The association's air as it last went: what its next
                // frames' least grants assume.
                if let Some(rate) = exchange.rate {
                    self.air.reported(key, rate);
                }
                Ok(())
            }
            // Settled when its association left: its return closes it.
            Some(Flight::Closed) => Ok(()),
            None => Err(AirtimeFault::UnknownTicket),
        }
    }
}

#[cfg(test)]
mod tests;
