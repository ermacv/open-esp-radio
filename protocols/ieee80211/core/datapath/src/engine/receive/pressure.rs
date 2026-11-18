//! The pressure rule (`design/engine-model-rx.py`, `tasks/205`): what waits
//! for the network, in the order of the moment each unit began to wait.
//!
//! The core knows nothing of a unit but its handle `H`. What it cannot know
//! it is told as values: what a unit needs of the network ([`Need`], read
//! by the round from the unit's bytes), whether its link is held
//! ([`Link`]), whether the data flows ([`Flow`]), how it holds its place
//! ([`Taken`]) and the places the reorder retains. It answers with a sum
//! that carries the handle back ([`Decision`]), or keeps the handle: every
//! call that takes a handle gives it back or keeps it in the queue. The
//! network's room is spent here: spending it is the rule's decision.
//!
//! - A unit is delivered when nothing waits and the network has room: lent
//!   when it may be, else copied; else it waits behind, in the order of its
//!   moment.
//! - A newcomer from the ring keeps a place (lent or waiting) only while a
//!   place stays free after it, the one the control plane passes through:
//!   at the last place it is copied, when nothing waits and the network can
//!   copy it, or refused for no place to wait. A unit the reorder lets go
//!   waits, or is lent, in the place it already holds.
//! - A unit that waited `late` from its own moment is refused at the head,
//!   every late head in one round; a held link's unit is refused when it
//!   reaches the head; a stopping data's goes back.

use heapless::Deque;
use oer_portable_time::{Duration, Instant};

use super::{CopyPermits, Flow, LendPermit, Room, RxRefusal};
use oer_protocols_ieee80211_contracts_port::PLACES;
use oer_protocols_ieee80211_core_upper_mac::block_ack::RetainRoom;

/// What a unit needs of the network, read by the round from its bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Need {
    /// One MSDU the network may borrow in place, or copy.
    Lendable,
    /// Frames to copy, each one (an A-MSDU's MSDUs, or a unit the network
    /// may not borrow).
    Copies(u8),
    /// Its bytes do not decapsulate: it is refused for the reason when it
    /// is delivered (behind what waits, in its order).
    Unread(RxRefusal),
}

/// Whether a unit's link carries its data, as the controller left it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Link {
    Carrying,
    /// The controller holds it (or the frame is of no link of ours).
    Held,
}

/// How a unit offered to the network holds its place.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Taken {
    /// It held a place before this round: the reorder let it go.
    InPlace,
    /// A newcomer from the ring, its place counted from what is taken
    /// outside the core (the reorder's retained, `outside`) before it
    /// joined.
    Newcomer { outside: usize },
}

/// What the core decided of a unit, the handle with it; or that it keeps
/// the unit.
#[derive(Debug)]
pub(super) enum Decision<H> {
    /// Lend it to the network: one unit more the network holds.
    Lend(H, LendPermit),
    /// Copy its frames into the network's storage; its place comes back at
    /// the round's close.
    Copy(H, CopyPermits),
    /// Refused, for the reason: back to the ring, counted.
    Refused(H, RxRefusal),
    /// Back to the ring unread: the data stops.
    Back(H),
    /// The core keeps it: it waits (a unit offered), or still waits at the
    /// head (the drain ends).
    Kept,
}

/// The round's side of the rule: what it reads of a unit (its need, its
/// link) and how it carries out a decision. A loan it could not make is
/// said, and the core takes the loan back.
pub(super) trait Hands<H> {
    fn read(&mut self, unit: &H) -> (Need, Link);
    fn carry(&mut self, decision: Decision<H>) -> Carried;
}

/// What carrying out a decision did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Carried {
    Done,
    /// A loan the core decided was not made: the unit was refused instead.
    LoanUnmade,
}

/// The pressure rule's state: the units that wait, each with its moment,
/// oldest first; how long one may wait; the units the network holds.
#[derive(Clone, Debug)]
pub(super) struct Pressure<H> {
    waiting: Deque<(H, Instant), PLACES>,
    late: Duration,
    lent: u8,
}

impl<H> Pressure<H> {
    pub(super) const fn new(late: Duration) -> Self {
        Self {
            waiting: Deque::new(),
            late,
            lent: 0,
        }
    }

    /// The network gave `reclaimed` units back.
    pub(super) fn reclaimed(&mut self, reclaimed: usize) {
        self.lent = self
            .lent
            .saturating_sub(u8::try_from(reclaimed).unwrap_or(u8::MAX));
    }

    /// The units that wait.
    #[cfg(test)]
    pub(super) fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// The units the network holds.
    #[cfg(test)]
    pub(super) const fn lent(&self) -> u8 {
        self.lent
    }

    /// The waiting units, oldest first, each with its moment.
    #[cfg(test)]
    pub(super) fn queue(&self) -> impl Iterator<Item = &(H, Instant)> {
        self.waiting.iter()
    }

    /// The core's state for a search that merges equal states: each waiting
    /// unit's name (`name`), with the order of its deadline among the
    /// waiting and `gap`'s place in the same order (the values go, the
    /// order stays), and the units lent.
    #[cfg(test)]
    pub(super) fn key(
        &self,
        gap: Option<Instant>,
        name: impl Fn(&H) -> u8,
    ) -> std::string::String {
        let deadlines = || {
            self.waiting
                .iter()
                .map(|(_, since)| since.checked_add(self.late))
        };
        let mut order: std::vec::Vec<Option<Instant>> = deadlines().chain([gap]).collect();
        if gap.is_none() {
            order.pop();
        }
        order.sort_unstable();
        order.dedup();
        let rank = |at: Option<Instant>| order.iter().position(|seen| *seen == at);
        let waiting: std::vec::Vec<(u8, Option<usize>)> = self
            .waiting
            .iter()
            .zip(deadlines())
            .map(|((unit, _), at)| (name(unit), rank(at)))
            .collect();
        let gap = gap.and_then(|gap| rank(Some(gap)));
        std::format!("{waiting:?}|gap {gap:?}|lent {}", self.lent)
    }


    /// The head's bound: its moment and `late`.
    pub(super) fn bound(&self) -> Option<Instant> {
        self.waiting
            .front()
            .and_then(|(_, since)| since.checked_add(self.late))
    }

    /// Whether the reorder may retain one more frame: a place stays free
    /// for the frame that closes the hole, beside the `retained`, the lent
    /// and the waiting, of `staging` places.
    pub(super) fn retain_room(&self, retained: usize, staging: usize) -> RetainRoom {
        if self.kept(retained).saturating_add(1) < staging {
            RetainRoom::Free
        } else {
            RetainRoom::Full
        }
    }

    /// What waits, from its head, until the head still waits: each head
    /// decided with what `read` finds of it (its need, its link), each
    /// decision given to `carry`.
    pub(super) fn drain(
        &mut self,
        flow: Flow,
        now: Instant,
        room: &mut Room,
        hands: &mut impl Hands<H>,
    ) {
        while let Some((head, _)) = self.waiting.front() {
            let (need, link) = hands.read(head);
            match self.next(need, link, flow, now, room) {
                Decision::Kept => break,
                decision => self.carry(decision, hands),
            }
        }
    }

    /// A decision carried out by the round; a loan it could not make comes
    /// back off the count.
    fn carry(&mut self, decision: Decision<H>, hands: &mut impl Hands<H>) {
        if hands.carry(decision) == Carried::LoanUnmade {
            self.lent = self.lent.saturating_sub(1);
        }
    }

    /// The head, decided with what the round read of it: a stopping data's
    /// goes back; a held link's and a late one are refused; else it is
    /// delivered while the network has room, or still waits.
    fn next(
        &mut self,
        need: Need,
        link: Link,
        flow: Flow,
        now: Instant,
        room: &mut Room,
    ) -> Decision<H> {
        let Some((handle, since)) = self.waiting.pop_front() else {
            return Decision::Kept;
        };
        if flow == Flow::Stopping {
            return Decision::Back(handle);
        }
        if link == Link::Held {
            return Decision::Refused(handle, RxRefusal::LinkHeld);
        }
        if now
            .checked_duration_since(since)
            .is_some_and(|waited| waited >= self.late)
        {
            return Decision::Refused(handle, RxRefusal::Late);
        }
        match self.deliver(handle, need, Place::Held, room) {
            Delivered::Decided(decision) => decision,
            Delivered::Waits(handle) => match self.waiting.push_front((handle, since)) {
                Ok(()) => Decision::Kept,
                // One was just taken from it: it has its place.
                Err((handle, _)) => Decision::Refused(handle, RxRefusal::Place),
            },
        }
    }

    /// A unit for the network in its order: a held link's refused; behind
    /// a unit that waits it waits too (the room has two dimensions: a
    /// later unit could be lent where the waiting one cannot be copied),
    /// else it is delivered; a newcomer at the last free place keeps no
    /// place.
    pub(super) fn offer(
        &mut self,
        handle: H,
        taken: Taken,
        staging: usize,
        now: Instant,
        room: &mut Room,
        hands: &mut impl Hands<H>,
    ) {
        let (need, link) = hands.read(&handle);
        let decision = self.decide(handle, need, link, taken, staging, now, room);
        self.carry(decision, hands);
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the unit, its need, link and place, the port's places, the clock and the room"
    )]
    fn decide(
        &mut self,
        handle: H,
        need: Need,
        link: Link,
        taken: Taken,
        staging: usize,
        now: Instant,
        room: &mut Room,
    ) -> Decision<H> {
        if link == Link::Held {
            return Decision::Refused(handle, RxRefusal::LinkHeld);
        }
        let place = match taken {
            Taken::Newcomer { outside } if self.kept(outside).saturating_add(1) >= staging => {
                Place::Last
            }
            Taken::InPlace | Taken::Newcomer { .. } => Place::Held,
        };
        let waits = if self.waiting.is_empty() {
            match self.deliver(handle, need, place, room) {
                Delivered::Decided(decision) => return decision,
                Delivered::Waits(handle) => handle,
            }
        } else {
            handle
        };
        if place == Place::Last {
            return Decision::Refused(waits, RxRefusal::NoPlaceToWait);
        }
        match self.waiting.push_back((waits, now)) {
            Ok(()) => Decision::Kept,
            // A place a unit: the queue has one for each.
            Err((handle, _)) => Decision::Refused(handle, RxRefusal::Place),
        }
    }

    /// The places the core's units take, beside `outside`.
    fn kept(&self, outside: usize) -> usize {
        outside
            .saturating_add(usize::from(self.lent))
            .saturating_add(self.waiting.len())
    }

    /// Lend while the network has room to borrow (a unit at the last place
    /// keeps none: not a loan), else copy while it has room to copy, else
    /// it waits.
    fn deliver(&mut self, handle: H, need: Need, place: Place, room: &mut Room) -> Delivered<H> {
        if let (Need::Lendable, Place::Held) = (need, place)
            && let Some(permit) = room.lend()
        {
            self.lent = self.lent.saturating_add(1);
            return Delivered::Decided(Decision::Lend(handle, permit));
        }
        let copies = match need {
            Need::Lendable => 1,
            Need::Copies(count) => usize::from(count),
            Need::Unread(reason) => return Delivered::Decided(Decision::Refused(handle, reason)),
        };
        match room.copies(copies) {
            Some(permits) => Delivered::Decided(Decision::Copy(handle, permits)),
            None => Delivered::Waits(handle),
        }
    }
}

/// Where a unit offered holds its place: one it held, or the last free one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Place {
    Held,
    Last,
}

/// What delivering a unit found.
enum Delivered<H> {
    Decided(Decision<H>),
    Waits(H),
}

#[cfg(test)]
mod tests;
