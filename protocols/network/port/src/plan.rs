//! What an egress policy keeps of one collection of the ready work, for the
//! frames that follow it: the continuation of its choice, bounded by
//! [`QUANTUM`].

use crate::frame::{Class, Ready};

/// The most frames one plan holds: a service's publications in one turn.
pub const QUANTUM: usize = 8;

/// What a plan's next step is.
#[derive(Debug, Eq, PartialEq)]
pub enum Next<C, R> {
    /// Send this work: the reservation is this frame's own, and exactly one
    /// of the policy's `accepted`, `refused` or `unknown` answers it.
    Send(C, R),
    /// Nothing may go now, as a choice over the same ready work would say:
    /// until a port's wake or a notice.
    Idle,
    /// The plan no longer knows what a choice over the ready work would
    /// say. Nothing was reserved; a new collection chooses.
    Stop(PlanStop),
}

/// Why a plan stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanStop {
    /// It sent what it held, and more work than it held was ready.
    Spent,
    /// What the choice reads besides the work moved: the link (its state,
    /// its association or its rate), a peer's sleep, or the turn of a peer's
    /// flows.
    Changed,
    /// The largest least grant of the account it serves went with the frames
    /// it sent, and the plan does not hold the next largest.
    Grant,
    /// The scheduler serves another account than the one whose turns it
    /// holds.
    Account,
}

/// Where a work stands in the turns: control first, as listed, then each
/// flow's works one a round, flows in turn after the last served.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Position {
    data: bool,
    round: u8,
    distance: u8,
    listed: usize,
}

/// One flow whose works the plan counts.
#[derive(Clone, Copy, Debug)]
struct Flow {
    flow: u8,
    distance: u8,
    /// Its works seen so far: the next one's round, at most [`QUANTUM`].
    seen: u8,
}

/// The first `Q` works of a choice that serves control first, in the order
/// listed, then flows in turn, one work of each a round, each flow's works in
/// the order listed: what such a choice sends next if every frame before is
/// accepted and nothing else moves. One pass over the ready work offers each
/// once; the plan keeps the `Q` first, and the flows they need, and nothing
/// else.
///
/// Flows take turns after `last`: the flow after it first, it last. A turn
/// that follows the list's own order ([`Self::listed`]) starts at the first
/// data work's flow, as a list that starts at the flow whose turn it is.
#[derive(Debug)]
pub struct Turns<T, const Q: usize> {
    last: Option<u8>,
    /// The first data work offered sets `last`.
    from_list: bool,
    planned: [Option<(Position, T)>; Q],
    flows: [Option<Flow>; Q],
    listed: usize,
    /// Some ready work offered is not in the plan.
    more: bool,
}

impl<T, const Q: usize> Turns<T, Q> {
    /// Turns after `last`, the flow served before; `None` starts at flow 0.
    pub const fn after(last: Option<u8>) -> Self {
        const {
            if Q == 0 || Q > u8::MAX as usize {
                panic!("a plan holds one to 255 works");
            }
        }
        Self {
            last,
            from_list: false,
            planned: [const { None }; Q],
            flows: [const { None }; Q],
            listed: 0,
            more: false,
        }
    }

    /// Turns from the first data work's flow, as the list gives them.
    pub const fn listed() -> Self {
        let mut turns = Self::after(None);
        turns.from_list = true;
        turns
    }

    /// Start again in place, as [`Self::after`] `last` starts: what it held
    /// is gone, and nothing of it moves.
    pub fn restart(&mut self, last: Option<u8>) {
        self.last = last;
        self.from_list = false;
        for slot in &mut self.planned {
            *slot = None;
        }
        for flow in &mut self.flows {
            *flow = None;
        }
        self.listed = 0;
        self.more = false;
    }

    /// Start again in place, as [`Self::listed`] starts.
    pub fn relist(&mut self) {
        self.restart(None);
        self.from_list = true;
    }

    /// Offer one ready work, `item`, described by `ready`, in the order the
    /// ready work lists them.
    pub fn offer(&mut self, ready: &Ready, item: T) {
        let listed = self.listed;
        self.listed = listed.saturating_add(1);
        let flow = ready.flow.unwrap_or(0);
        if ready.class == Class::Data && self.from_list {
            self.from_list = false;
            self.last = Some(flow.wrapping_sub(1));
        }
        let distance = self
            .last
            .map_or(flow, |last| flow.wrapping_sub(last).wrapping_sub(1));
        let position = if ready.class == Class::Control {
            Position {
                data: false,
                round: 0,
                distance,
                listed,
            }
        } else {
            let Some(round) = self.round(flow, distance) else {
                self.more = true;
                return;
            };
            Position {
                data: true,
                round,
                distance,
                listed,
            }
        };
        self.place(position, item);
    }

    /// The round of `flow`'s next work, counted; `None` when no work of it
    /// can be among the first `Q`.
    fn round(&mut self, flow: u8, distance: u8) -> Option<u8> {
        if let Some(known) = self
            .flows
            .iter_mut()
            .flatten()
            .find(|known| known.flow == flow)
        {
            let round = known.seen;
            // A work after `Q` of its own flow is not among the first `Q`.
            if usize::from(round) >= Q {
                return None;
            }
            known.seen = round.saturating_add(1);
            return Some(round);
        }
        let fresh = Flow {
            flow,
            distance,
            seen: 1,
        };
        if let Some(free) = self.flows.iter_mut().find(|slot| slot.is_none()) {
            *free = Some(fresh);
            return Some(0);
        }
        // `Q` flows nearer in turn each have a first work before any of the
        // farthest one's: that flow goes, with its works, or this one does.
        let farthest = self
            .flows
            .iter_mut()
            .flatten()
            .max_by_key(|known| known.distance)?;
        if farthest.distance < distance {
            return None;
        }
        let gone = farthest.distance;
        *farthest = fresh;
        self.more = true;
        for slot in &mut self.planned {
            if slot
                .as_ref()
                .is_some_and(|(position, _)| position.data && position.distance == gone)
            {
                *slot = None;
            }
        }
        Some(0)
    }

    /// Keep `item` at `position` if it is among the first `Q`.
    fn place(&mut self, position: Position, item: T) {
        if let Some(free) = self.planned.iter_mut().find(|slot| slot.is_none()) {
            *free = Some((position, item));
            return;
        }
        self.more = true;
        let last = self
            .planned
            .iter()
            .enumerate()
            .filter_map(|(place, slot)| slot.as_ref().map(|(at, _)| (*at, place)))
            .max();
        if let Some((at, place)) = last
            && position < at
            && let Some(slot) = self.planned.get_mut(place)
        {
            *slot = Some((position, item));
        }
    }

    fn first_place(&self) -> Option<usize> {
        self.planned
            .iter()
            .enumerate()
            .filter_map(|(place, slot)| slot.as_ref().map(|(at, _)| (*at, place)))
            .min()
            .map(|(_, place)| place)
    }

    /// The work next in turn.
    pub fn first(&self) -> Option<&T> {
        let place = self.first_place()?;
        self.planned
            .get(place)
            .and_then(Option::as_ref)
            .map(|(_, item)| item)
    }

    /// Take the work next in turn.
    pub fn take(&mut self) -> Option<T> {
        let place = self.first_place()?;
        self.planned
            .get_mut(place)
            .and_then(Option::take)
            .map(|(_, item)| item)
    }

    /// Whether some ready work offered is not in the plan: once the plan is
    /// taken, a choice would still find work.
    pub const fn more(&self) -> bool {
        self.more
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "tests compare a plan with a reference choice over fixed lists"
)]
mod tests;
