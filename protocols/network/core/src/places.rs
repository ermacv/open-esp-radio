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

use crate::block::{Block, Pool};
use crate::machine::{Outcome, Remote};

#[derive(Debug)]
pub(crate) struct Sending<'m, B, const SIZE: usize> {
    pub(crate) remote: Remote,
    pub(crate) block: Block<'m, B, SIZE>,
    pub(crate) since: Instant,
    /// The neighbor it goes through; broadcast has none. Its station address
    /// is read when the frame is built, so a change reaches a waiting datagram.
    pub(crate) hop: Option<[u8; 4]>,
}

/// A producer's work: a place and no block, its data the producer's until
/// it fills the slot.
#[derive(Debug)]
pub(crate) struct Declared<T> {
    pub(crate) remote: Remote,
    pub(crate) length: usize,
    /// As [`Sending::hop`].
    pub(crate) hop: Option<[u8; 4]>,
    /// Its producer's tag, from its declaration until its result is given:
    /// the producer keeps no record of its own.
    pub(crate) tag: T,
}

#[derive(Debug)]
pub(crate) enum Run<'m, B, const SIZE: usize, T> {
    Free,
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
    pub(crate) const fn hop(&self) -> Option<[u8; 4]> {
        match self {
            Self::Sending(Sending { hop, .. }) | Self::Declared(Declared { hop, .. }) => *hop,
            Self::Free | Self::Finished(_) | Self::Ended { .. } => None,
        }
    }

    /// Its result, once it ended.
    pub(crate) const fn outcome(&self) -> Option<Outcome> {
        match self {
            Self::Finished(outcome) | Self::Ended { outcome, .. } => Some(*outcome),
            Self::Free | Self::Sending(_) | Self::Declared(_) => None,
        }
    }

    /// A declared work's producer tag, while it runs or waits as a result.
    pub(crate) const fn tag(&self) -> Option<&T> {
        match self {
            Self::Declared(Declared { tag, .. }) | Self::Ended { tag, .. } => Some(tag),
            Self::Free | Self::Sending(_) | Self::Finished(_) => None,
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

/// A chain's two ends.
#[derive(Clone, Copy, Debug, Default)]
struct Line {
    head: Option<u8>,
    tail: Option<u8>,
}

/// One command's place: what it runs and, after, its result.
#[derive(Debug)]
pub(crate) struct Place<'m, B, const SIZE: usize, T> {
    /// The running command's, or 0.
    serial: u64,
    /// Someone holds the right to the result.
    observed: bool,
    run: Run<'m, B, SIZE, T>,
    /// The chain it is on, and its neighbors there.
    on: Option<Chain>,
    previous: Option<u8>,
    next: Option<u8>,
}

impl<'m, B, const SIZE: usize, T> Place<'m, B, SIZE, T> {
    const fn new() -> Self {
        Self {
            serial: 0,
            observed: false,
            run: Run::Free,
            on: None,
            previous: None,
            next: None,
        }
    }

    pub(crate) const fn serial(&self) -> u64 {
        self.serial
    }

    pub(crate) const fn run(&self) -> &Run<'m, B, SIZE, T> {
        &self.run
    }

    /// A declared work's producer tag.
    pub(crate) const fn tag(&self) -> Option<&T> {
        self.run.tag()
    }

    /// Its generation, its state and whether its result is observed, as it
    /// leaves its core.
    pub(crate) fn into_parts(self) -> (u64, bool, Run<'m, B, SIZE, T>) {
        (self.serial, self.observed, self.run)
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
            free: Line {
                head: None,
                tail: None,
            },
            running: Line {
                head: None,
                tail: None,
            },
            ended: Line {
                head: None,
                tail: None,
            },
        }
    }

    /// A place by its number: `CREDITS` is the close's.
    pub(crate) fn get(&self, number: u8) -> Option<&Place<'m, B, SIZE, T>> {
        let number = usize::from(number);
        if number == CREDITS {
            Some(&self.close)
        } else {
            self.places.get(number)
        }
    }

    /// A place by its number, while it runs `serial`'s command.
    pub(crate) fn of(&self, number: u8, serial: u64) -> Option<&Place<'m, B, SIZE, T>> {
        self.get(number).filter(|place| place.serial == serial)
    }

    fn get_mut(&mut self, number: u8) -> Option<&mut Place<'m, B, SIZE, T>> {
        let number = usize::from(number);
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
    fn push(&mut self, chain: Chain, number: u8) {
        let tail = self.ends(chain).tail;
        let Some(place) = self.get_mut(number) else {
            return;
        };
        place.on = Some(chain);
        place.previous = tail;
        place.next = None;
        match tail.and_then(|tail| self.get_mut(tail)) {
            Some(last) => last.next = Some(number),
            None => self.ends(chain).head = Some(number),
        }
        self.ends(chain).tail = Some(number);
    }

    /// Take `number` off the chain it is on, if any.
    fn unlink(&mut self, number: u8) {
        let Some(place) = self.get_mut(number) else {
            return;
        };
        let Some(chain) = place.on.take() else {
            return;
        };
        let (previous, next) = (place.previous.take(), place.next.take());
        match previous.and_then(|previous| self.get_mut(previous)) {
            Some(before) => before.next = next,
            None => self.ends(chain).head = next,
        }
        match next.and_then(|next| self.get_mut(next)) {
            Some(after) => after.previous = previous,
            None => self.ends(chain).tail = previous,
        }
    }

    /// The one write of a place's state: its chain follows it. An ended
    /// command goes on the ended chain only from a running one, so its end
    /// is told once.
    fn set(&mut self, number: u8, run: Run<'m, B, SIZE, T>) -> Option<Run<'m, B, SIZE, T>> {
        let close = usize::from(number) == CREDITS;
        let place = self.get_mut(number)?;
        let was_running = place.run.running();
        let to = match &run {
            Run::Free if close => None,
            Run::Free => Some(Chain::Free),
            Run::Sending(_) | Run::Declared(_) => Some(Chain::Running),
            Run::Finished(_) | Run::Ended { .. } if was_running || close => Some(Chain::Ended),
            // Told already, or never told: its chain stays.
            Run::Finished(_) | Run::Ended { .. } => place.on,
        };
        let old = core::mem::replace(&mut place.run, run);
        let on = place.on;
        if on != to {
            self.unlink(number);
            if let Some(chain) = to {
                self.push(chain, number);
            }
        }
        Some(old)
    }

    /// The number of a free place, which [`Self::start`] takes next.
    pub(crate) fn vacant(&self) -> Option<u8> {
        self.free.head.or_else(|| {
            (self.fresh < CREDITS)
                .then(|| u8::try_from(self.fresh).ok())
                .flatten()
        })
    }

    /// Run `run` as `serial`'s command in the place [`Self::vacant`] named.
    /// Whether it was still free.
    pub(crate) fn start(&mut self, number: u8, serial: u64, run: Run<'m, B, SIZE, T>) -> bool {
        if self.vacant() != Some(number) {
            return false;
        }
        if usize::from(number) == self.fresh && self.free.head.is_none() {
            self.fresh = self.fresh.saturating_add(1);
        }
        let Some(place) = self.get_mut(number) else {
            return false;
        };
        place.observed = true;
        place.serial = serial;
        let _free = self.set(number, run);
        true
    }

    /// End the command in `number` as `outcome`, if it runs, its block to
    /// `pool`; whether it held a neighbor's protection, which ends with it.
    pub(crate) fn end<const BLOCKS: usize>(
        &mut self,
        number: u8,
        outcome: Outcome,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) -> bool {
        let Some(place) = self.get(number).filter(|place| place.run.running()) else {
            return false;
        };
        let protected = place.run.hop().is_some();
        if let Some(Run::Sending(sending)) = self.finish(number, outcome) {
            pool.give(sending.block);
        }
        protected
    }

    /// End declared work in `number` as `outcome`: it holds no block.
    /// Whether it held a neighbor's protection.
    pub(crate) fn end_declared(&mut self, number: u8, outcome: Outcome) -> bool {
        let Some(Run::Declared(declared)) = self.get(number).map(Place::run) else {
            return false;
        };
        let protected = declared.hop.is_some();
        let _declared = self.finish(number, outcome);
        protected
    }

    /// End declared `number`, which its port took, as `outcome` given to its
    /// producer at once: the place frees now, with no end to tell, and its
    /// tag comes back with whether it held a neighbor's protection. Any
    /// other running command ends as [`Self::end`] ends it, and no tag
    /// comes back.
    pub(crate) fn end_given<const BLOCKS: usize>(
        &mut self,
        number: u8,
        outcome: Outcome,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) -> (bool, Option<T>) {
        let given = self
            .get(number)
            .filter(|place| place.observed)
            .and_then(|place| match &place.run {
                Run::Declared(declared) => Some(declared.tag),
                _ => None,
            });
        let Some(tag) = given else {
            return (self.end(number, outcome, pool), None);
        };
        let protected = self
            .get(number)
            .is_some_and(|place| place.run.hop().is_some());
        let _declared = self.free(number);
        (protected, Some(tag))
    }

    /// End `Declined` every declared command whose tag `keep` no longer
    /// keeps: how many ended, and whether one held a neighbor's protection.
    pub(crate) fn withdraw_declared(&mut self, mut keep: impl FnMut(&T) -> bool) -> (usize, bool) {
        let (mut ended, mut released) = (0_usize, false);
        let mut at = self.running.head;
        for _ in 0..=CREDITS {
            let Some(number) = at else {
                break;
            };
            let Some(place) = self.get(number) else {
                break;
            };
            at = place.next;
            let stale = match &place.run {
                Run::Declared(declared) => !keep(&declared.tag),
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
    fn finish(&mut self, number: u8, outcome: Outcome) -> Option<Run<'m, B, SIZE, T>> {
        let place = self.get(number)?;
        if place.observed {
            // A declared work's result keeps its tag, for its producer.
            let result = match &place.run {
                Run::Declared(declared) => Run::Ended {
                    outcome,
                    tag: declared.tag,
                },
                _ => Run::Finished(outcome),
            };
            self.set(number, result)
        } else {
            self.free(number)
        }
    }

    fn free(&mut self, number: u8) -> Option<Run<'m, B, SIZE, T>> {
        let old = self.set(number, Run::Free);
        if let Some(place) = self.get_mut(number) {
            place.serial = 0;
        }
        old
    }

    /// Every running command whose neighbor is `address` ends `outcome`.
    pub(crate) fn end_through<const BLOCKS: usize>(
        &mut self,
        address: Option<[u8; 4]>,
        outcome: Outcome,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) -> bool {
        let mut released = false;
        let mut at = self.running.head;
        for _ in 0..=CREDITS {
            let Some(number) = at else {
                break;
            };
            let Some(place) = self.get(number) else {
                break;
            };
            at = place.next;
            if address.is_none_or(|address| place.run.hop() == Some(address)) {
                released |= self.end(number, outcome, pool);
            }
        }
        released
    }

    /// Every running command ends `outcome`; whether one held a neighbor.
    pub(crate) fn end_all<const BLOCKS: usize>(
        &mut self,
        outcome: Outcome,
        pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    ) -> bool {
        self.end_through(None, outcome, pool)
    }

    /// The close finished at once, as `serial`'s command: its end is told as
    /// a command's.
    pub(crate) fn close(&mut self, serial: u64) {
        let number = u8::try_from(CREDITS).unwrap_or(u8::MAX);
        self.close.observed = true;
        self.close.serial = serial;
        let _free = self.set(number, Run::Finished(Outcome::SocketClosed));
    }

    /// The result of `serial`'s command in `number`, once it ended: the
    /// place frees. `None` names no such command; `Some(None)` one that runs.
    pub(crate) fn take(&mut self, number: u8, serial: u64) -> Option<Option<Outcome>> {
        let place = self.of(number, serial)?;
        let Some(outcome) = place.run.outcome() else {
            return Some(None);
        };
        let _finished = self.free(number);
        Some(Some(outcome))
    }

    /// Nobody will read `serial`'s result: a finished command's place frees
    /// now, a running one at its end.
    pub(crate) fn unobserve(&mut self, number: u8, serial: u64) {
        let Some(place) = self.get_mut(number).filter(|place| place.serial == serial) else {
            return;
        };
        place.observed = false;
        if place.run.outcome().is_some() {
            let _finished = self.free(number);
        }
    }

    /// The next end not yet told, oldest first, as its place and its
    /// generation: each end is told once, and its result stays in its place.
    pub(crate) fn tell(&mut self) -> Option<(u8, u64)> {
        let number = self.ended.head?;
        let serial = self.get(number)?.serial;
        self.unlink(number);
        Some((number, serial))
    }

    /// The next end not yet told of a declared work, oldest first, with its
    /// generation, outcome and tag: its place frees, so its result is given
    /// once. An end of other work on the way is told and stays for its
    /// holder.
    pub(crate) fn tell_declared(&mut self) -> Option<(u8, u64, Outcome, T)> {
        for _ in 0..=CREDITS {
            let (number, serial) = self.tell()?;
            let Some(place) = self.get(number) else {
                continue;
            };
            if let Run::Ended { outcome, tag } = place.run {
                let _finished = self.free(number);
                return Some((number, serial, outcome, tag));
            }
        }
        None
    }

    /// The running commands, oldest first.
    pub(crate) fn running(
        &self,
    ) -> impl Iterator<Item = (u8, &Place<'m, B, SIZE, T>)> + Clone + '_ {
        let mut at = self.running.head;
        // A chain is at most every place long.
        (0..=CREDITS).map_while(move |_| {
            let number = at?;
            let place = self.get(number)?;
            at = place.next;
            Some((number, place))
        })
    }

    /// Whether a command runs: the running chain has a head. Every place a
    /// walk of [`Self::running`] visits is on that chain.
    pub(crate) const fn runs(&self) -> bool {
        self.running.head.is_some()
    }

    /// Whether every place is free and nothing waits to be read.
    pub(crate) fn idle(&self) -> bool {
        matches!(self.close.run, Run::Free)
            && self
                .places
                .iter()
                .all(|place| matches!(place.run, Run::Free))
    }

    /// The places, as their core leaves them.
    pub(crate) fn into_places(self) -> ([Place<'m, B, SIZE, T>; CREDITS], Place<'m, B, SIZE, T>) {
        (self.places, self.close)
    }
}
