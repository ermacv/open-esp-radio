//! The pressure rule's own search: every order of what the rule is told
//! (rounds, newcomers from the ring, frames the reorder retains and lets
//! go, the network's returns and copy room, the clock, the controller's
//! hold, the data's stop), over cloned states of the core, a number for a
//! unit's handle.
//!
//! The world keeps its own record, apart from the core's: where each unit
//! is (waiting with its moment, retained by the reorder, held by the
//! network, gone and how), the network's room, the link, the order units
//! were offered in. The properties are the model's
//! (`design/engine-model-rx.py`), by its numbers, as far as they are the
//! rule's; the round's (P2, P4, P5, P10–P12) stay with the engine's search.
//! The world's count of what the network holds is checked against the
//! core's after every step (`tasks/213`). States merge by their whole key;
//! the waiting units' moments enter it as their age at the world's clock,
//! which is what the rule reads.

use super::*;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::format;
use std::string::String;
use std::vec::Vec;

/// How long a unit may wait, in the search's microseconds.
const LATE: Duration = Duration::from_micros(2);

/// What a unit that arrives needs, in the order units arrive: a loan, an
/// A-MSDU of two, two loans, one that does not decapsulate, a copy, a
/// loan, a loan.
const ARRIVALS: [Need<()>; 8] = [
    Need::Lendable(()),
    Need::Copies(2),
    Need::Lendable(()),
    Need::Lendable(()),
    Need::Unread(RxRefusal::Undecapsulable),
    Need::Copies(1),
    Need::Lendable(()),
    Need::Lendable(()),
];

/// What the search may still do.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct Bounds {
    /// Units that arrive, of [`ARRIVALS`].
    units: usize,
    /// Units the reorder may retain (each let go later, in its order).
    retains: usize,
    /// Lent units the network gives back.
    returns: usize,
    /// Copies the network consumes, each giving its room back.
    frees: usize,
    /// Microseconds the clock may move by one (the head's bound is a move
    /// of its own).
    ticks: usize,
    /// Holds of the link (each may be released).
    holds: usize,
    /// Stops of the data.
    stops: usize,
}

/// Where a unit went, by the world's record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Gone {
    Copied,
    /// Lent, and given back by the network.
    Returned,
    Refused(RxRefusal),
    Back,
}

/// The world: the core and its own record.
#[derive(Clone, Debug)]
struct World {
    core: Pressure<u8>,
    staging: usize,
    loans: u8,
    now: Instant,
    /// The units that wait, oldest first, each with its moment.
    waits: Vec<(u8, Instant)>,
    /// The reorder's, in their order.
    retained: Vec<u8>,
    /// The units the network holds.
    lent: BTreeSet<u8>,
    /// Units the network gave back since the last round.
    returned: usize,
    /// What the network may copy.
    copy: u8,
    link: Link,
    flow: Flow,
    gone: Vec<(u8, Gone)>,
    /// The units offered to the core, in the order of the reorder (the
    /// order data must keep).
    offered: Vec<u8>,
    /// The place in `offered` of the last unit delivered.
    delivered: Option<usize>,
    arrived: usize,
    left: Bounds,
    /// What the search reached, for its coverage (not in the key).
    reached: Reached,
}

/// The cases the search must reach.
#[derive(Clone, Copy, Debug, Default)]
struct Reached {
    /// Offers of a unit that does not decapsulate while one waits.
    unread_behind: usize,
    /// Offers of a unit that does not decapsulate at the last place.
    unread_at_last: usize,
}

/// One step of a path.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// A round: the head first, then what the ring and the reorder give.
    Round(Then),
    Return(u8),
    Free,
    Tick,
    Bound,
    Hold,
    Release,
    Stop,
}

/// What a round is given after the head.
#[derive(Clone, Copy, Debug)]
enum Then {
    Nothing,
    /// The next unit from the ring, offered as a newcomer.
    Newcomer,
    /// The next unit from the ring, retained by the reorder.
    Retained,
    /// The next unit from the ring, offered as a newcomer, then every
    /// retained one let go in its place: the frame that closes the hole.
    Closes,
}

/// What the world found wrong.
type Problems = Vec<&'static str>;

impl World {
    fn new(staging: usize, loans: u8, left: Bounds) -> Self {
        Self {
            core: Pressure::new(LATE),
            staging,
            loans,
            now: Instant::from_micros(1_000),
            waits: Vec::new(),
            retained: Vec::new(),
            lent: BTreeSet::new(),
            returned: 0,
            copy: 1,
            link: Link::Carrying,
            flow: Flow::Flowing,
            gone: Vec::new(),
            offered: Vec::new(),
            delivered: None,
            arrived: 0,
            left,
            reached: Reached::default(),
        }
    }

    fn need(unit: u8) -> Need<()> {
        ARRIVALS
            .get(usize::from(unit))
            .copied()
            .unwrap_or(Need::Lendable(()))
    }

    fn places_taken(&self) -> usize {
        self.retained.len() + self.lent.len() + self.waits.len()
    }

    fn steps(&self) -> Vec<Step> {
        let mut steps = Vec::new();
        if self.flow == Flow::Stopping {
            return steps;
        }
        steps.push(Step::Round(Then::Nothing));
        if self.arrived < self.left.units {
            steps.push(Step::Round(Then::Newcomer));
            if self.left.retains > 0 && self.link == Link::Carrying {
                steps.push(Step::Round(Then::Retained));
            }
            if !self.retained.is_empty() {
                steps.push(Step::Round(Then::Closes));
            }
        }
        if self.left.returns > 0 {
            steps.extend(self.lent.iter().map(|unit| Step::Return(*unit)));
        }
        if self.left.frees > 0 {
            steps.push(Step::Free);
        }
        if self.left.ticks > 0 {
            steps.push(Step::Tick);
        }
        if self.core.bound().is_some_and(|bound| bound > self.now) {
            steps.push(Step::Bound);
        }
        match self.link {
            Link::Carrying if self.left.holds > 0 => steps.push(Step::Hold),
            Link::Held => steps.push(Step::Release),
            Link::Carrying => {}
        }
        if self.left.stops > 0 {
            steps.push(Step::Stop);
        }
        steps
    }

    fn apply(&mut self, step: Step, problems: &mut Problems) {
        match step {
            Step::Round(then) => self.round(then, problems),
            Step::Return(unit) => {
                self.left.returns -= 1;
                self.lent.remove(&unit);
                self.gone.push((unit, Gone::Returned));
                self.returned += 1;
            }
            Step::Free => {
                self.left.frees -= 1;
                self.copy = self.copy.saturating_add(1);
            }
            Step::Tick => {
                self.left.ticks -= 1;
                self.now = self.now.saturating_add(Duration::from_micros(1));
            }
            Step::Bound => {
                if let Some(bound) = self.core.bound() {
                    self.now = bound;
                }
            }
            Step::Hold => {
                self.left.holds -= 1;
                self.link = Link::Held;
            }
            Step::Release => self.link = Link::Carrying,
            Step::Stop => {
                self.left.stops -= 1;
                self.flow = Flow::Stopping;
                // The reorder's go back with the stop.
                for unit in core::mem::take(&mut self.retained) {
                    self.gone.push((unit, Gone::Back));
                }
                self.round(Then::Nothing, problems);
            }
        }
    }

    /// A round: the returns told, the head drained, then what the round is
    /// given, each decision carried out and checked.
    fn round(&mut self, then: Then, problems: &mut Problems) {
        self.core.reclaimed(core::mem::take(&mut self.returned));
        let lend = self
            .loans
            .saturating_sub(u8::try_from(self.lent.len()).unwrap_or(u8::MAX));
        let mut room = Room::new(lend, self.copy);
        while let Some(head) = self.core.head().copied() {
            let need = Self::need(head);
            let decision = self
                .core
                .next(need, self.link, self.flow, self.now, &mut room);
            if matches!(decision, Decision::Kept) {
                break;
            }
            self.carried(head, decision, Origin::Head, problems);
        }
        match then {
            Then::Nothing => {}
            Then::Newcomer => {
                let unit = self.arrive();
                self.offer(unit, &mut room, problems);
            }
            Then::Retained => {
                let unit = self.arrive();
                self.left.retains -= 1;
                if self.core.retain_room(self.retained.len(), self.staging) == RetainRoom::Free {
                    self.retained.push(unit);
                } else {
                    // The reorder found no room: its window moves, and the
                    // unit and what it retained are let go.
                    self.closes(unit, &mut room, problems);
                }
            }
            Then::Closes => {
                let unit = self.arrive();
                self.closes(unit, &mut room, problems);
            }
        }
        self.copy = room.copy;
        self.closed(&room, problems);
    }

    /// `unit` closes the hole: offered as a newcomer, then what the reorder
    /// retained, each in its place.
    fn closes(&mut self, unit: u8, room: &mut Room, problems: &mut Problems) {
        self.offer(unit, room, problems);
        for unit in core::mem::take(&mut self.retained) {
            let decision = self.core.offer(
                unit,
                Self::need(unit),
                self.link,
                Taken::InPlace,
                self.staging,
                self.now,
                room,
            );
            self.carried(unit, decision, Origin::InPlace, problems);
        }
    }

    fn arrive(&mut self) -> u8 {
        let unit = u8::try_from(self.arrived).unwrap_or(u8::MAX);
        self.arrived += 1;
        unit
    }

    fn offer(&mut self, unit: u8, room: &mut Room, problems: &mut Problems) {
        let outside = self.retained.len();
        let last = self.places_taken() + 1 >= self.staging;
        if let Need::Unread(_) = Self::need(unit) {
            self.reached.unread_behind += usize::from(!self.waits.is_empty());
            self.reached.unread_at_last += usize::from(last);
        }
        let decision = self.core.offer(
            unit,
            Self::need(unit),
            self.link,
            Taken::Newcomer { outside },
            self.staging,
            self.now,
            room,
        );
        self.carried(unit, decision, Origin::Newcomer { last }, problems);
    }

    /// A decision about `unit`, carried out on the world's record, and the
    /// properties of a single decision checked.
    fn carried(
        &mut self,
        unit: u8,
        decision: Decision<u8, ()>,
        origin: Origin,
        problems: &mut Problems,
    ) {
        let head = matches!(origin, Origin::Head);
        let moment = if head && self.waits.first().map(|(first, _)| *first) == Some(unit) {
            Some(self.waits.remove(0).1)
        } else if head {
            problems.push("P1 the core's head is not the world's oldest waiting unit");
            None
        } else {
            None
        };
        let handle = match &decision {
            Decision::Lend(handle, (), _)
            | Decision::Copy(handle, _)
            | Decision::Refused(handle, _)
            | Decision::Back(handle) => Some(*handle),
            Decision::Kept => None,
        };
        if handle.is_some_and(|handle| handle != unit) {
            problems.push("P1 the core gave back another unit than it decided");
        }
        if !head {
            self.offered.push(unit);
        }
        if let Need::Unread(own) = Self::need(unit)
            && !matches!(decision, Decision::Refused(_, reason) if reason == own)
        {
            problems.push(
                "P7 a unit that does not decapsulate was not refused at once for its own reason",
            );
        }
        let delivered = matches!(decision, Decision::Lend(..) | Decision::Copy(..));
        if delivered && self.link == Link::Held {
            problems.push("P9 data was delivered while the link is held");
        }
        if delivered {
            let rank = self.offered.iter().position(|offered| *offered == unit);
            if rank < self.delivered {
                problems.push("P3 data left out of its order");
            }
            self.delivered = rank;
        }
        if !head && delivered && !self.waits.is_empty() {
            problems.push("P3 a unit was delivered past one that waits");
        }
        match decision {
            Decision::Lend(unit, (), _) => {
                if let Origin::Newcomer { last: true } = origin {
                    problems.push("P6 a newcomer was lent at the last place");
                }
                self.lent.insert(unit);
            }
            Decision::Copy(unit, permits) => {
                let copies = match Self::need(unit) {
                    Need::Lendable(()) => 1,
                    Need::Copies(count) => count,
                    Need::Unread(_) => {
                        problems.push("P7 a unit that does not decapsulate was copied");
                        0
                    }
                };
                if permits.0 != copies {
                    problems.push("P1 a unit was copied with other than its frames' permits");
                }
                self.gone.push((unit, Gone::Copied));
            }
            Decision::Refused(unit, reason) => {
                match reason {
                    RxRefusal::NoPlaceToWait
                        if !matches!(origin, Origin::Newcomer { last: true }) =>
                    {
                        problems
                            .push("P7 a unit was refused for no place to wait with places free");
                    }
                    RxRefusal::Late
                        if !moment.is_some_and(|since| {
                            self.now
                                .checked_duration_since(since)
                                .is_some_and(|waited| waited >= LATE)
                        }) =>
                    {
                        problems.push(
                            "P7 a unit was refused late before it waited LATE from its own moment",
                        );
                    }
                    RxRefusal::LinkHeld if self.link == Link::Carrying => {
                        problems.push("P7 a unit was refused for a hold that was not");
                    }
                    RxRefusal::Undecapsulable
                        if Self::need(unit) != Need::Unread(RxRefusal::Undecapsulable) =>
                    {
                        problems.push("P7 a unit was refused for bytes it does not have");
                    }
                    RxRefusal::Place
                    | RxRefusal::Unverified
                    | RxRefusal::AmsduTooLong
                    | RxRefusal::Reorder => {
                        problems.push("P7 a refusal without its reason");
                    }
                    _ => {}
                }
                if self.flow == Flow::Stopping {
                    problems.push("P13 the stop refused what waited instead of giving it back");
                }
                self.gone.push((unit, Gone::Refused(reason)));
            }
            Decision::Back(unit) => {
                if self.flow == Flow::Flowing {
                    problems.push("P13 a unit went back unread while the data flows");
                }
                self.gone.push((unit, Gone::Back));
            }
            Decision::Kept => {
                if head {
                    problems.push("P1 the drain ended on a head it took");
                }
                if let Origin::Newcomer { last: true } = origin {
                    problems.push("P6 a newcomer waits at the last place");
                }
                self.waits.push((unit, self.now));
            }
        }
    }

    /// The properties of a round's close, and of every state.
    fn closed(&self, room: &Room, problems: &mut Problems) {
        if self.flow == Flow::Flowing && self.places_taken() >= self.staging {
            problems.push("P6 a round closed with no place free");
        }
        if self.waits.iter().any(|(_, since)| {
            self.now
                .checked_duration_since(*since)
                .is_some_and(|waited| waited >= LATE)
        }) {
            problems.push("P8 a unit that waited LATE from its own moment still waits");
        }
        if let Some((head, _)) = self.waits.first() {
            let can = match Self::need(*head) {
                Need::Lendable(()) => room.lend > 0 || room.copy > 0,
                Need::Copies(count) => room.copy >= count,
                Need::Unread(_) => true,
            };
            if can && self.link == Link::Carrying {
                problems.push("P14 a unit waits for a network with room to lend or to copy");
            }
        }
        if self.flow == Flow::Stopping && !self.waits.is_empty() {
            problems.push("P13 the data stopped with units waiting for the network");
        }
    }

    /// The properties of every state, the world's record against the core.
    fn check(&self, problems: &mut Problems) {
        let mut seen: Vec<u8> = self
            .waits
            .iter()
            .map(|(unit, _)| *unit)
            .chain(self.retained.iter().copied())
            .chain(self.lent.iter().copied())
            .chain(self.gone.iter().map(|(unit, _)| *unit))
            .collect();
        seen.sort_unstable();
        if seen != (0..u8::try_from(self.arrived).unwrap_or(u8::MAX)).collect::<Vec<_>>() {
            problems.push("P1 a unit is in two places or in none");
        }
        if !self
            .core
            .queue()
            .map(|(unit, since)| (*unit, *since))
            .eq(self.waits.iter().copied())
        {
            problems.push("P1 the core's waiting units are not the world's");
        }
        // `tasks/213`: the lent count is the core's, and the world's own.
        if usize::from(self.core.lent()) != self.lent.len() + self.returned {
            problems.push("P1 the core's count of lent units is not the world's");
        }
        if self.lent.len() > usize::from(self.loans) {
            problems.push("P1 the network holds more than its limit");
        }
        if self.waits.windows(2).any(|pair| pair[0].1 > pair[1].1) {
            problems.push("P3 the waiting queue is out of the order of its moments");
        }
        let bound = self
            .waits
            .first()
            .and_then(|(_, since)| since.checked_add(LATE));
        if self.core.bound() != bound {
            problems.push("P14 the head's bound is not its own");
        }
    }

    fn key(&self) -> String {
        let ages: Vec<(u8, u64)> = self
            .waits
            .iter()
            .map(|(unit, since)| {
                (
                    *unit,
                    self.now.saturating_duration_since(*since).as_micros(),
                )
            })
            .collect();
        let mut gone: Vec<String> = self.gone.iter().map(|gone| format!("{gone:?}")).collect();
        gone.sort_unstable();
        format!(
            "{ages:?}|{}|{:?}|{:?}|{}|{}|{:?}|{:?}|{:?}|{:?}|{}|{:?}",
            self.core.lent(),
            self.retained,
            self.lent,
            self.returned,
            self.copy,
            self.link,
            self.flow,
            gone,
            self.delivered,
            self.arrived,
            self.left,
        )
    }
}

/// Where a decided unit came from.
#[derive(Clone, Copy, Debug)]
enum Origin {
    Head,
    InPlace,
    /// A newcomer, `last` when the world finds it at the last free place.
    Newcomer {
        last: bool,
    },
}

/// What a search found: the states it passed, or a problem and its path.
struct Found {
    states: usize,
    reached: Reached,
    problem: Option<(&'static str, Vec<Step>)>,
}

fn search(staging: usize, loans: u8, left: Bounds, ceiling: usize) -> Found {
    let mut seen = HashSet::new();
    let mut reached = Reached::default();
    let mut stack = std::vec![(World::new(staging, loans, left), Vec::new())];
    while let Some((world, path)) = stack.pop() {
        if !seen.insert(world.key()) {
            continue;
        }
        assert!(
            seen.len() <= ceiling,
            "the search passed its ceiling of {ceiling} states"
        );
        for step in world.steps() {
            let mut next = world.clone();
            next.reached = Reached::default();
            let mut problems = Problems::new();
            next.apply(step, &mut problems);
            next.check(&mut problems);
            reached.unread_behind += next.reached.unread_behind;
            reached.unread_at_last += next.reached.unread_at_last;
            let mut path = path.clone();
            path.push(step);
            if let Some(problem) = problems.first() {
                return Found {
                    states: seen.len(),
                    reached,
                    problem: Some((problem, path)),
                };
            }
            stack.push((next, path));
        }
    }
    Found {
        states: seen.len(),
        reached,
        problem: None,
    }
}

fn run(name: &str, staging: usize, loans: u8, left: Bounds, ceiling: usize) {
    let clock = std::time::Instant::now();
    let found = search(staging, loans, left, ceiling);
    std::eprintln!(
        "pressure search {name}: {} states, {:?}",
        found.states,
        clock.elapsed()
    );
    if let Some((problem, path)) = found.problem {
        panic!("PROBLEM {problem}\n    by {path:?}");
    }
    assert!(
        found.reached.unread_behind > 0 && found.reached.unread_at_last > 0,
        "the search did not reach a unit that does not decapsulate behind a waiting one and at the last place: {:?}",
        found.reached
    );
}

/// The model's numbers: four places, one loan.
const STAGING: usize = 4;
const LOANS: u8 = 1;

#[test]
fn every_order_the_rule_is_told_keeps_its_properties() {
    run(
        "model",
        STAGING,
        LOANS,
        Bounds {
            units: 6,
            retains: 2,
            returns: 2,
            frees: 2,
            ticks: 2,
            holds: 1,
            stops: 1,
        },
        2_000_000,
    );
}

#[test]
fn every_order_with_two_loans_keeps_its_properties() {
    run(
        "two loans",
        STAGING,
        2,
        Bounds {
            units: 6,
            retains: 2,
            returns: 2,
            frees: 1,
            ticks: 2,
            holds: 1,
            stops: 1,
        },
        2_000_000,
    );
}

/// The world at the last place: one lent and two retained leave one place
/// of four.
fn at_the_last_place(copy: u8) -> World {
    let mut world = World::new(
        STAGING,
        LOANS,
        Bounds {
            units: 4,
            retains: 2,
            returns: 0,
            frees: 0,
            ticks: 0,
            holds: 0,
            stops: 0,
        },
    );
    let mut problems = Problems::new();
    for step in [
        Step::Round(Then::Newcomer),
        Step::Round(Then::Retained),
        Step::Round(Then::Retained),
    ] {
        world.apply(step, &mut problems);
        world.check(&mut problems);
    }
    assert_eq!(problems, Problems::new());
    assert_eq!(world.places_taken(), STAGING - 1);
    world.copy = copy;
    world
}

/// The search reaches the last place: a newcomer that could be lent is
/// copied there, and with no copy room refused for no place to wait.
#[test]
fn the_search_reaches_the_last_place() {
    for (copy, gone) in [
        (1, Gone::Copied),
        (0, Gone::Refused(RxRefusal::NoPlaceToWait)),
    ] {
        let mut world = at_the_last_place(copy);
        let mut problems = Problems::new();
        world.apply(Step::Round(Then::Newcomer), &mut problems);
        world.check(&mut problems);
        assert_eq!(problems, Problems::new());
        assert_eq!(world.gone.last(), Some(&(3, gone)));
        assert_eq!(world.places_taken(), STAGING - 1);
    }
}
