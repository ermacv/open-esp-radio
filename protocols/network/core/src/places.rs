//! A socket's command places: each work's one record, and the chains that
//! index the records by state.
//!
//! A place is the only record of its work: its data source (a block, or a
//! producer's declared length), its generation (the serial) and its state.
//! Every place but the close's is on exactly one chain while it is free,
//! running or ended and not yet told, and on none once its end was told and
//! its result waits for its holder: the chain follows the state, and only a
//! transition here moves it. So a place is never on two chains, an end is
//! told once, and a walk over running or ended work reads only that work.
//! A chain holds place numbers; who reads one checks the generation against
//! its own right, a [`CommandId`](crate::CommandId).

use oer_portable_time::Instant;
use oer_protocols_network_wire::address::Ipv4Address;

use crate::block::{Block, Pool};
use crate::machine::{Outcome, Remote};
use crate::serial::Serial;

#[derive(Debug)]
pub(crate) struct Sending<'m, B, const SIZE: usize> {
    pub(crate) remote: Remote,
    pub(crate) block: Block<'m, B, SIZE>,
    pub(crate) since: Instant,
    /// The neighbor it goes through; broadcast has none. Its station address
    /// is read when the frame is built, so a change reaches a waiting datagram.
    pub(crate) hop: Option<Ipv4Address>,
}

/// A producer's work: a place and no block, its data the producer's until
/// it fills the slot.
#[derive(Debug)]
pub(crate) struct Declared<T> {
    pub(crate) remote: Remote,
    pub(crate) length: usize,
    /// As [`Sending::hop`].
    pub(crate) hop: Option<Ipv4Address>,
    /// Its producer's tag, from its declaration until its result is given:
    /// the producer keeps no record of its own.
    pub(crate) tag: T,
}

#[derive(Debug)]
pub(crate) enum Run<'m, B, const SIZE: usize, T> {
    Sending(Sending<'m, B, SIZE>),
    Declared(Declared<T>),
    /// A command's result.
    Finished(Outcome),
    /// A declared work's result, with its tag.
    Ended {
        outcome: Outcome,
        tag: T,
    },
}

impl<B, const SIZE: usize, T> Run<'_, B, SIZE, T> {
    /// The neighbor running work goes through, which it protects.
    pub(crate) const fn hop(&self) -> Option<Ipv4Address> {
        match self {
            Self::Sending(Sending { hop, .. }) | Self::Declared(Declared { hop, .. }) => *hop,
            Self::Finished(_) | Self::Ended { .. } => None,
        }
    }

    /// Its result, once it ended.
    pub(crate) const fn outcome(&self) -> Option<Outcome> {
        match self {
            Self::Finished(outcome) | Self::Ended { outcome, .. } => Some(*outcome),
            Self::Sending(_) | Self::Declared(_) => None,
        }
    }

    /// A declared work's producer tag, while it runs or waits as a result.
    pub(crate) const fn tag(&self) -> Option<&T> {
        match self {
            Self::Declared(Declared { tag, .. }) | Self::Ended { tag, .. } => Some(tag),
            Self::Sending(_) | Self::Finished(_) => None,
        }
    }

    pub(crate) const fn running(&self) -> bool {
        matches!(self, Self::Sending(_) | Self::Declared(_))
    }
}

/// The chains a place can be on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Chain {
    /// Free for a next command: places freed before, in the order they freed.
    Free,
    /// Sending or declared, in the order they were accepted.
    Running,
    /// Ended and not yet told, in the order they ended.
    Ended,
}

/// A place's number among its socket's places, which only the places give:
/// `CREDITS` is the close's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PlaceNumber(u8);

impl PlaceNumber {
    /// Its index among the places.
    pub(crate) const fn index(self) -> usize {
        self.0 as usize
    }

    /// The numbers of places in their order, from the first: those a
    /// socket's places left in a retired core's results.
    pub(crate) fn in_order() -> impl Iterator<Item = Self> {
        (0_u8..=u8::MAX).map(Self)
    }
}

/// A chain's two ends.
#[derive(Clone, Copy, Debug)]
enum Line {
    Empty,
    Of {
        head: PlaceNumber,
        tail: PlaceNumber,
    },
}

impl Line {
    const fn head(self) -> Option<PlaceNumber> {
        match self {
            Self::Empty => None,
            Self::Of { head, .. } => Some(head),
        }
    }

    const fn tail(self) -> Option<PlaceNumber> {
        match self {
            Self::Empty => None,
            Self::Of { tail, .. } => Some(tail),
        }
    }

    /// With `number` behind its tail.
    const fn push(self, number: PlaceNumber) -> Self {
        match self {
            Self::Empty => Self::Of {
                head: number,
                tail: number,
            },
            Self::Of { head, .. } => Self::Of { head, tail: number },
        }
    }

    /// Without a place whose neighbors were `previous` and `next`.
    const fn without(self, previous: Option<PlaceNumber>, next: Option<PlaceNumber>) -> Self {
        match (self, previous, next) {
            (Self::Empty, _, _) | (_, None, None) => Self::Empty,
            (Self::Of { tail, .. }, None, Some(head)) => Self::Of { head, tail },
            (Self::Of { head, .. }, Some(tail), None) => Self::Of { head, tail },
            (line @ Self::Of { .. }, Some(_), Some(_)) => line,
        }
    }
}

/// Where a place stands among the chains.
#[derive(Clone, Copy, Debug)]
enum Link {
    Off,
    /// On `chain`, between these; none at its end.
    On {
        chain: Chain,
        previous: Option<PlaceNumber>,
        next: Option<PlaceNumber>,
    },
}

/// What a place holds: nothing, or one command with its number, whether
/// someone holds the right to its result, and its work or result.
#[derive(Debug)]
pub(crate) enum PlaceState<'m, B, const SIZE: usize, T> {
    Free,
    Occupied(Occupied<'m, B, SIZE, T>),
}

/// One command in its place.
#[derive(Debug)]
pub(crate) struct Occupied<'m, B, const SIZE: usize, T> {
    pub(crate) serial: Serial,
    /// Someone holds the right to the result.
    pub(crate) observed: bool,
    pub(crate) run: Run<'m, B, SIZE, T>,
}

/// One command's place: what it runs and, after, its result.
#[derive(Debug)]
pub(crate) struct Place<'m, B, const SIZE: usize, T> {
    state: PlaceState<'m, B, SIZE, T>,
    link: Link,
}

impl<'m, B, const SIZE: usize, T> Place<'m, B, SIZE, T> {
    const fn new() -> Self {
        Self {
            state: PlaceState::Free,
            link: Link::Off,
        }
    }

    /// The command it holds.
    const fn occupied(&self) -> Option<&Occupied<'m, B, SIZE, T>> {
        match &self.state {
            PlaceState::Free => None,
            PlaceState::Occupied(occupied) => Some(occupied),
        }
    }

    /// Its command's number, while it holds one.
    pub(crate) const fn serial(&self) -> Option<Serial> {
        match self.occupied() {
            Some(occupied) => Some(occupied.serial),
            None => None,
        }
    }

    /// Whether it holds `serial`'s command.
    fn holds(&self, serial: Serial) -> bool {
        self.serial() == Some(serial)
    }

    /// Whether someone holds the right to its command's result.
    fn observed(&self) -> bool {
        self.occupied().is_some_and(|occupied| occupied.observed)
    }

    /// The chain it is on.
    const fn chain(&self) -> Option<Chain> {
        match self.link {
            Link::Off => None,
            Link::On { chain, .. } => Some(chain),
        }
    }

    /// The place behind it on its chain.
    const fn next(&self) -> Option<PlaceNumber> {
        match self.link {
            Link::Off => None,
            Link::On { next, .. } => next,
        }
    }

    /// Its command's work or result, while it holds one.
    pub(crate) const fn run(&self) -> Option<&Run<'m, B, SIZE, T>> {
        match self.occupied() {
            Some(occupied) => Some(&occupied.run),
            None => None,
        }
    }

    /// Whether its command runs.
    fn running(&self) -> bool {
        self.run().is_some_and(Run::running)
    }

    /// The neighbor its running command goes through.
    fn hop(&self) -> Option<Ipv4Address> {
        self.run().and_then(Run::hop)
    }

    /// A declared work's producer tag.
    pub(crate) fn tag(&self) -> Option<&T> {
        self.run().and_then(Run::tag)
    }

    /// What it holds, as it leaves its core.
    pub(crate) fn into_state(self) -> PlaceState<'m, B, SIZE, T> {
        self.state
    }
}

/// A place [`Places::vacant`] found free: the right to start one command in
/// it, before anything else starts there.
#[derive(Debug)]
pub(crate) struct Vacant(PlaceNumber);

impl Vacant {
    pub(crate) const fn number(&self) -> PlaceNumber {
        self.0
    }
}

/// A socket's places: `CREDITS` for commands and one for its close, whose
/// number is `CREDITS`.
#[derive(Debug)]
pub(crate) struct Places<'m, B, const SIZE: usize, const CREDITS: usize, T> {
    places: [Place<'m, B, SIZE, T>; CREDITS],
    close: Place<'m, B, SIZE, T>,
    /// Places from here on were never used: free, and on no chain.
    fresh: usize,
    free: Line,
    running: Line,
    ended: Line,
}

impl<'m, B, const SIZE: usize, const CREDITS: usize, T: Copy> Places<'m, B, SIZE, CREDITS, T> {
    pub(crate) const fn new() -> Self {
        Self {
            places: [const { Place::new() }; CREDITS],
            close: Place::new(),
            fresh: 0,
            free: Line::Empty,
            running: Line::Empty,
            ended: Line::Empty,
        }
    }

    /// A place by its number: `CREDITS` is the close's.
    pub(crate) fn get(&self, number: PlaceNumber) -> Option<&Place<'m, B, SIZE, T>> {
        let number = number.index();
        if number == CREDITS {
            Some(&self.close)
        } else {
            self.places.get(number)
        }
    }

    /// A place by its number, while it runs `serial`'s command.
    pub(crate) fn of(&self, number: PlaceNumber, serial: Serial) -> Option<&Place<'m, B, SIZE, T>> {
        self.get(number).filter(|place| place.holds(serial))
    }

    fn get_mut(&mut self, number: PlaceNumber) -> Option<&mut Place<'m, B, SIZE, T>> {
        let number = number.index();
        if number == CREDITS {
            Some(&mut self.close)
        } else {
            self.places.get_mut(number)
        }
    }

    const fn ends(&mut self, chain: Chain) -> &mut Line {
        match chain {
            Chain::Free => &mut self.free,
            Chain::Running => &mut self.running,
            Chain::Ended => &mut self.ended,
        }
    }

    /// Put `number` at `chain`'s tail. It is on no chain.
    fn push(&mut self, chain: Chain, number: PlaceNumber) {
        let line = *self.ends(chain);
        let Some(place) = self.get_mut(number) else {
            return;
        };
        place.link = Link::On {
            chain,
            previous: line.tail(),
            next: None,
        };
        if let Some(Link::On { next, .. }) = self.link_of(line.tail()) {
            *next = Some(number);
        }
        *self.ends(chain) = line.push(number);
    }

    /// Take `number` off the chain it is on, if any.
    fn unlink(&mut self, number: PlaceNumber) {
        let Some(place) = self.get_mut(number) else {
            return;
        };
        let Link::On {
            chain,
            previous,
            next,
        } = place.link
        else {
            return;
        };
        place.link = Link::Off;
        if let Some(Link::On { next: after, .. }) = self.link_of(previous) {
            *after = next;
        }
        if let Some(Link::On {
            previous: before, ..
        }) = self.link_of(next)
        {
            *before = previous;
        }
        let line = self.ends(chain);
        *line = line.without(previous, next);
    }

    /// The link of the place `number`, if there is one.
    fn link_of(&mut self, number: Option<PlaceNumber>) -> Option<&mut Link> {
        self.get_mut(number?).map(|place| &mut place.link)
    }

    /// Write a place's state and hand the old one back: its chain follows
    /// it, as with [`Self::put`], which writes one in place when nothing of
    /// the old is needed. An ended command goes on the ended chain only from
    /// a running one, so its end is told once.
    fn set(
        &mut self,
        number: PlaceNumber,
        state: PlaceState<'m, B, SIZE, T>,
    ) -> Option<PlaceState<'m, B, SIZE, T>> {
        let to = self.chain_for(number, &state)?;
        let place = self.get_mut(number)?;
        let old = core::mem::replace(&mut place.state, state);
        self.relink(number, to);
        Some(old)
    }

    /// [`Self::set`] where the place's state before holds no block (it is
    /// free, declared or a result): the new state is written in place and
    /// nothing of the old is moved out. A sending command's end, whose block
    /// goes back to the pool, takes [`Self::set`].
    fn put(&mut self, number: PlaceNumber, state: PlaceState<'m, B, SIZE, T>) {
        let Some(to) = self.chain_for(number, &state) else {
            return;
        };
        if let Some(place) = self.get_mut(number) {
            place.state = state;
        }
        self.relink(number, to);
    }

    /// Free `number`, whose state holds no block, as [`Self::free`] does.
    fn free_unsent(&mut self, number: PlaceNumber) {
        self.put(number, PlaceState::Free);
    }

    /// The chain `number` goes on with `run`, as the place stands now:
    /// `None` names no such place; `Some(None)` no chain.
    fn chain_for(
        &self,
        number: PlaceNumber,
        state: &PlaceState<'m, B, SIZE, T>,
    ) -> Option<Option<Chain>> {
        let close = number.index() == CREDITS;
        let place = self.get(number)?;
        let was_running = place.running();
        Some(match state {
            PlaceState::Free if close => None,
            PlaceState::Free => Some(Chain::Free),
            PlaceState::Occupied(occupied) => match occupied.run {
                Run::Sending(_) | Run::Declared(_) => Some(Chain::Running),
                Run::Finished(_) | Run::Ended { .. } if was_running || close => Some(Chain::Ended),
                // Told already, or never told: its chain stays.
                Run::Finished(_) | Run::Ended { .. } => place.chain(),
            },
        })
    }

    /// Put `number` on `to`, off the chain it was on.
    fn relink(&mut self, number: PlaceNumber, to: Option<Chain>) {
        let on = self.get(number).and_then(Place::chain);
        if on != to {
            self.unlink(number);
            if let Some(chain) = to {
                self.push(chain, number);
            }
        }
    }

    /// The close's number, when it is one.
    pub(crate) fn close_number() -> Option<PlaceNumber> {
        u8::try_from(CREDITS).ok().map(PlaceNumber)
    }

    /// A free place, which [`Self::start`] takes.
    pub(crate) fn vacant(&self) -> Option<Vacant> {
        self.free
            .head()
            .or_else(|| {
                (self.fresh < CREDITS)
                    .then(|| u8::try_from(self.fresh).ok().map(PlaceNumber))
                    .flatten()
            })
            .map(Vacant)
    }

    /// Run `run` as `serial`'s command in `vacant`.
    pub(crate) fn start(&mut self, vacant: Vacant, serial: Serial, run: Run<'m, B, SIZE, T>) {
        let Vacant(number) = vacant;
        // With no freed place, the vacant one is the first never used.
        if self.free.head().is_none() {
            self.fresh = self.fresh.saturating_add(1);
        }
        // The place was free: nothing of it is moved out.
        self.put(
            number,
            PlaceState::Occupied(Occupied {
                serial,
                observed: true,
                run,
            }),
        );
    }

    /// End the command in `number` as `outcome`, if it runs, its block to
    /// `pool`; whether it held a neighbor's protection, which ends with it.
    pub(crate) fn end<const BLOCKS: usize>(
        &mut self,
        number: PlaceNumber,
        outcome: Outcome,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) -> bool {
        let protected = self.get(number).is_some_and(|place| place.hop().is_some());
        self.end_running(number, outcome, pool);
        protected
    }

    /// As [`Self::end`], for who does not ask of the protection.
    fn end_running<const BLOCKS: usize>(
        &mut self,
        number: PlaceNumber,
        outcome: Outcome,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) {
        if self.get(number).is_some_and(Place::running)
            && let Some(PlaceState::Occupied(Occupied {
                run: Run::Sending(sending),
                ..
            })) = self.finish(number, outcome)
        {
            pool.give(sending.block);
        }
    }

    /// End declared work in `number` as `outcome`: it holds no block.
    /// Whether it held a neighbor's protection.
    pub(crate) fn end_declared(&mut self, number: PlaceNumber, outcome: Outcome) -> bool {
        let Some(Run::Declared(declared)) = self.get(number).and_then(Place::run) else {
            return false;
        };
        let protected = declared.hop.is_some();
        self.finish_unsent(number, outcome);
        protected
    }

    /// End declared `number`, which its port took, as `outcome` given to its
    /// producer at once: the place frees now, with no end to tell, and its
    /// tag comes back with whether it held a neighbor's protection. Any
    /// other running command ends as [`Self::end`] ends it, and no tag
    /// comes back.
    pub(crate) fn end_given<const BLOCKS: usize>(
        &mut self,
        number: PlaceNumber,
        outcome: Outcome,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) -> (bool, Option<T>) {
        let given = self
            .get(number)
            .filter(|place| place.observed())
            .and_then(|place| match place.run() {
                Some(Run::Declared(declared)) => Some(declared.tag),
                _ => None,
            });
        let Some(tag) = given else {
            return (self.end(number, outcome, pool), None);
        };
        let protected = self.get(number).is_some_and(|place| place.hop().is_some());
        self.free_unsent(number);
        (protected, Some(tag))
    }

    /// End `Declined` every declared command whose tag `keep` no longer
    /// keeps: how many ended, and whether one held a neighbor's protection.
    pub(crate) fn withdraw_declared(&mut self, mut keep: impl FnMut(&T) -> bool) -> (usize, bool) {
        let (mut ended, mut released) = (0_usize, false);
        let mut at = self.running.head();
        for _ in 0..=CREDITS {
            let Some(number) = at else {
                break;
            };
            let Some(place) = self.get(number) else {
                break;
            };
            at = place.next();
            let stale = match place.run() {
                Some(Run::Declared(declared)) => !keep(&declared.tag),
                _ => false,
            };
            if stale {
                ended = ended.saturating_add(1);
                released |= self.end_declared(number, Outcome::Declined);
            }
        }
        (ended, released)
    }

    /// End the command. A place nobody observes frees here; otherwise the
    /// result waits in it, and its end goes on the ended chain.
    fn finish(
        &mut self,
        number: PlaceNumber,
        outcome: Outcome,
    ) -> Option<PlaceState<'m, B, SIZE, T>> {
        match self.result(number, outcome)? {
            Some(result) => self.set(number, result),
            None => self.free(number),
        }
    }

    /// What `number`'s command leaves when it ends `outcome`: its result,
    /// for whoever observes it, or nothing to keep (`Some(None)`). `None`
    /// names no command.
    fn result(
        &self,
        number: PlaceNumber,
        outcome: Outcome,
    ) -> Option<Option<PlaceState<'m, B, SIZE, T>>> {
        let occupied = self.get(number)?.occupied()?;
        if !occupied.observed {
            return Some(None);
        }
        // A declared work's result keeps its tag, for its producer.
        let run = match &occupied.run {
            Run::Declared(declared) => Run::Ended {
                outcome,
                tag: declared.tag,
            },
            _ => Run::Finished(outcome),
        };
        Some(Some(PlaceState::Occupied(Occupied {
            serial: occupied.serial,
            observed: true,
            run,
        })))
    }

    /// [`Self::finish`] of a command whose state holds no block: a declared
    /// work's result keeps its tag, and nothing is moved out.
    fn finish_unsent(&mut self, number: PlaceNumber, outcome: Outcome) {
        match self.result(number, outcome) {
            Some(Some(result)) => self.put(number, result),
            Some(None) => self.free_unsent(number),
            None => {}
        }
    }

    fn free(&mut self, number: PlaceNumber) -> Option<PlaceState<'m, B, SIZE, T>> {
        self.set(number, PlaceState::Free)
    }

    /// Every running command whose neighbor is `address` ends `outcome`.
    pub(crate) fn end_through<const BLOCKS: usize>(
        &mut self,
        address: Option<Ipv4Address>,
        outcome: Outcome,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) {
        let mut at = self.running.head();
        for _ in 0..=CREDITS {
            let Some(number) = at else {
                break;
            };
            let Some(place) = self.get(number) else {
                break;
            };
            at = place.next();
            if address.is_none_or(|address| place.hop() == Some(address)) {
                self.end_running(number, outcome, pool);
            }
        }
    }

    /// Whether a running command protects a neighbor.
    pub(crate) fn protects(&self) -> bool {
        self.running().any(|(_, place)| place.hop().is_some())
    }

    /// Every running command ends `outcome`.
    pub(crate) fn end_all<const BLOCKS: usize>(
        &mut self,
        outcome: Outcome,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) {
        self.end_through(None, outcome, pool);
    }

    /// The close finished at once, as `serial`'s command: its end is told as
    /// a command's.
    pub(crate) fn close(&mut self, serial: Serial) {
        let number = PlaceNumber(u8::try_from(CREDITS).unwrap_or(u8::MAX));
        self.put(
            number,
            PlaceState::Occupied(Occupied {
                serial,
                observed: true,
                run: Run::Finished(Outcome::SocketClosed),
            }),
        );
    }

    /// The result of `serial`'s command in `number`, once it ended: the
    /// place frees. `None` names no such command; `Some(None)` one that runs.
    pub(crate) fn take(&mut self, number: PlaceNumber, serial: Serial) -> Option<Option<Outcome>> {
        let place = self.of(number, serial)?;
        let Some(outcome) = place.run().and_then(Run::outcome) else {
            return Some(None);
        };
        self.free_unsent(number);
        Some(Some(outcome))
    }

    /// Nobody will read `serial`'s result: a finished command's place frees
    /// now, a running one at its end.
    pub(crate) fn unobserve(&mut self, number: PlaceNumber, serial: Serial) {
        let Some(PlaceState::Occupied(occupied)) = self
            .get_mut(number)
            .filter(|place| place.holds(serial))
            .map(|place| &mut place.state)
        else {
            return;
        };
        occupied.observed = false;
        if occupied.run.outcome().is_some() {
            self.free_unsent(number);
        }
    }

    /// The next end not yet told, oldest first, as its place and its
    /// generation: each end is told once, and its result stays in its place.
    pub(crate) fn tell(&mut self) -> Option<(PlaceNumber, Serial)> {
        let number = self.ended.head()?;
        let serial = self.get(number)?.serial()?;
        self.unlink(number);
        Some((number, serial))
    }

    /// The next end not yet told of a declared work, oldest first, with its
    /// generation, outcome and tag: its place frees, so its result is given
    /// once. An end of other work on the way is told and stays for its
    /// holder.
    pub(crate) fn tell_declared(&mut self) -> Option<(PlaceNumber, Serial, Outcome, T)> {
        for _ in 0..=CREDITS {
            let (number, serial) = self.tell()?;
            let Some(place) = self.get(number) else {
                continue;
            };
            if let Some(&Run::Ended { outcome, tag }) = place.run() {
                self.free_unsent(number);
                return Some((number, serial, outcome, tag));
            }
        }
        None
    }

    /// The oldest running command's place: the running chain's head.
    pub(crate) const fn first_running(&self) -> Option<PlaceNumber> {
        self.running.head()
    }

    /// The running command after `number`'s on the running chain, with
    /// `number`'s place.
    pub(crate) fn running_at(
        &self,
        number: PlaceNumber,
    ) -> Option<(&Place<'m, B, SIZE, T>, Option<PlaceNumber>)> {
        let place = self.get(number)?;
        Some((place, place.next()))
    }

    /// The running commands, oldest first.
    pub(crate) fn running(
        &self,
    ) -> impl Iterator<Item = (PlaceNumber, &Place<'m, B, SIZE, T>)> + Clone + '_ {
        let mut at = self.running.head();
        // A chain is at most every place long.
        (0..=CREDITS).map_while(move |_| {
            let number = at?;
            let place = self.get(number)?;
            at = place.next();
            Some((number, place))
        })
    }

    /// Whether a command runs: the running chain has a head. Every place a
    /// walk of [`Self::running`] visits is on that chain.
    pub(crate) const fn runs(&self) -> bool {
        self.running.head().is_some()
    }

    /// Whether every place is free and nothing waits to be read.
    pub(crate) fn idle(&self) -> bool {
        matches!(self.close.state, PlaceState::Free)
            && self
                .places
                .iter()
                .all(|place| matches!(place.state, PlaceState::Free))
    }

    /// The places, as their core leaves them.
    pub(crate) fn into_places(self) -> ([Place<'m, B, SIZE, T>; CREDITS], Place<'m, B, SIZE, T>) {
        (self.places, self.close)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serial::Serials;

    type TestPlaces = Places<'static, (), 64, 2, u8>;

    fn declared<'m, B>(tag: u8) -> Run<'m, B, 64, u8> {
        Run::Declared(Declared {
            remote: Remote {
                address: Ipv4Address::new([192, 0, 2, 1]),
                port: 9,
            },
            length: 1,
            hop: None,
            tag,
        })
    }

    /// A free place holds no number; a started one its command's; an ended
    /// and observed one keeps it with its result until the result is taken,
    /// and the place is free again.
    #[test]
    fn a_place_holds_a_number_only_while_it_holds_a_command() {
        let mut places = TestPlaces::new();
        let mut serials = Serials::new();
        let vacant = places.vacant().unwrap();
        let number = vacant.number();
        assert_eq!(places.get(number).and_then(Place::serial), None);
        let serial = serials.next();
        places.start(vacant, serial, declared(7));
        assert_eq!(places.get(number).and_then(Place::serial), Some(serial));
        assert!(places.get(number).is_some_and(Place::running));
        assert!(!places.end_declared(number, Outcome::Accepted));
        // The result waits with its number; its end is told once.
        assert_eq!(places.tell(), Some((number, serial)));
        assert_eq!(places.tell(), None);
        assert_eq!(places.get(number).and_then(Place::serial), Some(serial));
        // Another number names no command here.
        assert_eq!(places.take(number, serials.next()), None);
        assert_eq!(places.take(number, serial), Some(Some(Outcome::Accepted)));
        assert_eq!(places.get(number).and_then(Place::serial), None);
        assert!(places.idle());
    }

    /// A command nobody observes frees its place at its end, with no result
    /// and no number left; one unobserved after its end frees at once.
    #[test]
    fn an_unobserved_command_leaves_no_number() {
        let mut places = TestPlaces::new();
        let mut serials = Serials::new();
        let vacant = places.vacant().unwrap();
        let number = vacant.number();
        let serial = serials.next();
        places.start(vacant, serial, declared(1));
        places.unobserve(number, serial);
        assert_eq!(places.get(number).and_then(Place::serial), Some(serial));
        places.end_declared(number, Outcome::Accepted);
        assert_eq!(places.get(number).and_then(Place::serial), None);
        assert_eq!(places.tell(), None);

        let vacant = places.vacant().unwrap();
        let number = vacant.number();
        let serial = serials.next();
        places.start(vacant, serial, declared(2));
        places.end_declared(number, Outcome::Accepted);
        places.unobserve(number, serial);
        assert_eq!(places.get(number).and_then(Place::serial), None);
        assert!(places.idle());
    }

    /// The close holds its command's number with its result until taken.
    #[test]
    fn the_close_holds_its_number_until_its_result_is_taken() {
        let mut places = TestPlaces::new();
        let serial = Serials::new().next();
        places.close(serial);
        let close = TestPlaces::close_number().unwrap();
        assert_eq!(places.get(close).and_then(Place::serial), Some(serial));
        assert!(!places.idle());
        assert_eq!(
            places.take(close, serial),
            Some(Some(Outcome::SocketClosed))
        );
        assert_eq!(places.get(close).and_then(Place::serial), None);
        assert!(places.idle());
    }

    /// A command that ended keeps its outcome: ending it again moves
    /// nothing, as only a running command ends.
    #[test]
    fn an_ended_command_keeps_its_outcome_when_ended_again() {
        crate::with_local_network(|right| {
            let mut storage = [const { crate::BlockPlace::<64>::new() }; 2];
            let (mut pool, _brand) = crate::claim(right, &mut storage);
            let mut places = Places::<'_, _, 64, 2, u8>::new();
            let serial = Serials::new().next();
            let vacant = places.vacant().unwrap();
            let number = vacant.number();
            places.start(vacant, serial, declared(3));
            places.end_declared(number, Outcome::Accepted);
            assert!(!places.end(number, Outcome::Unreachable, &mut pool));
            assert_eq!(places.take(number, serial), Some(Some(Outcome::Accepted)));
        });
    }
}
