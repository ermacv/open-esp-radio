//! Every order of a station engine's receive sections over the model port,
//! as `design/engine-model-rx.py` searches its model (C1: no receive
//! agreement, so no reorder): the ring's signal with a data or a
//! management frame arrived, the network's return of a lent unit, its copy
//! room changed, the episode's bound, the data's stop, a round owed.
//!
//! The network lends one unit at most and copies one frame a round while
//! its copy room is open; the model port has eight places. The world keeps
//! its own record: what arrived, what the network got and gave back, what
//! the controller read, when the first unit began to wait. Properties are
//! read through the engine's public API and what the model port and the
//! test's network show. States are merged by their whole key, the
//! engine's and the model port's included; the episode's instant is left
//! out: the search reaches the bound only by its own move.

use super::*;
use std::string::String;
use std::time::Instant as Clock;

/// The units the network may hold at once.
const LEND: u8 = 1;
/// The most steps of a path.
const DEPTH: usize = 24;

/// What the search may still do.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(super) struct Bounds {
    /// Frames that arrive, of the kinds in [`KINDS`]' order of counts.
    pub units: usize,
    /// Lent units the network gives back.
    pub returns: usize,
    /// Stops of the data.
    pub stops: usize,
    /// Copies the network consumes, each giving its room back.
    pub frees: usize,
}

/// The kinds of the frames that arrive: of the first `units`, how many of
/// each.
const KINDS: [Kind; 8] = [
    Kind::Data,
    Kind::Data,
    Kind::Data,
    Kind::Management,
    Kind::Data,
    Kind::Data,
    Kind::Management,
    Kind::Data,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum Kind {
    Data,
    Management,
}

/// One step of the search.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Move {
    /// A frame of the kind arrives: the ring's signal.
    Arrives(Kind),
    /// The network gives its lent unit at this index back.
    Returns(usize),
    /// The network consumed a copy: its storage takes one more, and it
    /// says so (its wake).
    Frees,
    /// The episode's bound comes.
    Late,
    /// The data's stop.
    Stop,
    /// A round owed.
    Round,
}

struct World<'id> {
    engine: StationEngine<'id, 2>,
    around: Around<'id, STATION_SINGLES, 0>,
    left: Bounds,
    /// Each arrived frame's kind, by its id less one.
    arrived: Vec<Kind>,
    /// The data ids the network got, in order.
    delivered: Vec<u8>,
    /// What of the network's and the controller's the world read so far.
    received_read: usize,
    frames_read: usize,
    /// Lent units given back.
    returned: usize,
    /// The stop asked.
    stopped: bool,
    /// The micros of the first wait of the episode, as the world saw it.
    first_wait: Option<u64>,
    owed: Rest,
    problems: Vec<&'static str>,
}

impl<'id> World<'id> {
    fn new(run: Running<'id, STATION_SINGLES, 0>, bounds: Bounds) -> Self {
        let mut engine = StationEngine::<'_, 2>::new(4);
        let mut around = station(run, &mut engine, 2, HOUR);
        around.net.lend_limit = LEND;
        around.net.copy_room = 0;
        // Two units a service, as the model's frontier.
        around.port.set_frontier(2);
        Self {
            engine,
            around,
            left: bounds,
            arrived: Vec::new(),
            delivered: Vec::new(),
            received_read: 0,
            frames_read: 0,
            returned: 0,
            stopped: false,
            first_wait: None,
            owed: Rest::Wake(None),
            problems: Vec::new(),
        }
    }

    fn kind_left(&self, kind: Kind) -> bool {
        let total = KINDS
            .iter()
            .take(self.arrived.len() + self.left.units)
            .filter(|k| **k == kind)
            .count();
        let arrived = self.arrived.iter().filter(|k| **k == kind).count();
        arrived < total
    }

    /// The places of the units that wait for the network: held, not lent.
    fn waiting_places(&self) -> Vec<u8> {
        let lent: Vec<u8> = self
            .around
            .net
            .lent
            .iter()
            .map(|(unit, _)| unit.place())
            .collect();
        self.around
            .port
            .held_places()
            .into_iter()
            .filter(|place| !lent.contains(place))
            .collect()
    }

    /// The units that wait for the network: held in a place, not lent.
    fn waiting(&self) -> usize {
        self.around
            .port
            .places_held()
            .saturating_sub(self.around.net.lent.len())
    }

    fn moves(&self) -> Vec<Move> {
        let mut moves = Vec::new();
        if self.left.units > 0 {
            for kind in [Kind::Data, Kind::Management] {
                if self.kind_left(kind) {
                    moves.push(Move::Arrives(kind));
                }
            }
        }
        if self.left.returns > 0 {
            moves.extend((0..self.around.net.lent.len()).map(Move::Returns));
        }
        if self.left.frees > 0 {
            moves.push(Move::Frees);
        }
        if self.waiting() > 0 && !self.stopped {
            moves.push(Move::Late);
        }
        if !self.stopped && self.left.stops > 0 {
            moves.push(Move::Stop);
        }
        if self.owed == Rest::Round {
            moves.push(Move::Round);
        }
        moves
    }

    fn apply(&mut self, action: Move) {
        let pressure = self.refusals().pressure;
        let waited = self.waiting_places();
        let services = self.around.port.counts().services;
        let rest = match action {
            Move::Arrives(kind) => {
                self.left.units -= 1;
                self.arrived.push(kind);
                let id = self.arrived.len() as u8;
                let frame = match kind {
                    Kind::Data => receive::data(u16::from(id), receive::IPV4, id),
                    Kind::Management => receive::beacon(u16::from(id)),
                };
                self.around.port.land(&frame, receive::meta());
                Some(self.engine.received(&mut self.around.turn()))
            }
            Move::Returns(index) => {
                self.left.returns -= 1;
                self.returned += 1;
                self.around.net.lent.remove(index);
                Some(self.engine.returned(&mut self.around.turn()))
            }
            Move::Frees => {
                self.left.frees -= 1;
                self.around.net.copy_room = self.around.net.copy_room.saturating_add(1);
                Some(self.engine.returned(&mut self.around.turn()))
            }
            Move::Late => {
                let bound = self
                    .first_wait
                    .map_or(0, |first| first + NETWORK_LATE.as_micros());
                if bound > self.around.micros {
                    self.around.micros = bound;
                }
                Some(self.engine.due(&mut self.around.turn()))
            }
            Move::Stop => {
                self.left.stops -= 1;
                self.stopped = true;
                let _ = station_flowing(&mut self.engine).stop(&mut self.around.turn());
                None
            }
            Move::Round => Some(self.engine.received(&mut self.around.turn())),
        };
        self.owed = rest.unwrap_or(Rest::Wake(None));
        self.read(action);
        // While a unit waits for the network, the ring is not serviced: a
        // unit that waited before the step and still waits after it says
        // the round was held back.
        let still = self
            .waiting_places()
            .iter()
            .any(|place| waited.contains(place));
        if still && self.around.port.counts().services > services && !self.stopped {
            self.problems.push("a held-back engine serviced the ring");
        }
        self.check(action, pressure);
    }

    fn refusals(&mut self) -> RxRefusals {
        match self.engine.data() {
            DataPlace::Flowing(flowing) => flowing.refusals(),
            _ => RxRefusals::default(),
        }
    }

    /// The world's record of what the network and the controller got.
    fn read(&mut self, action: Move) {
        let received = &self.around.net.received[self.received_read..];
        if self.stopped && !received.is_empty() && action != Move::Stop {
            self.problems
                .push("data was delivered after the data stopped");
        }
        for got in received {
            let tag = match got {
                Got::Copied(frame) => frame.last().copied(),
                Got::Lent(place) => self
                    .around
                    .port
                    .place_bytes(*place)
                    .and_then(|bytes| bytes.last().copied()),
            };
            match tag {
                Some(tag) => self.delivered.push(tag),
                None => self.problems.push("a delivery names no unit"),
            }
        }
        self.received_read = self.around.net.received.len();
        if self.stopped && self.around.told.frames.len() > self.frames_read && action != Move::Stop
        {
            self.problems
                .push("a frame was read after the data stopped");
        }
        self.frames_read = self.around.told.frames.len();
    }

    fn check(&mut self, action: Move, pressure_before: u16) {
        let now = self.around.micros;
        let waiting = self.waiting();
        if self.around.net.lent.len() > usize::from(LEND) {
            self.problems.push("the network holds more than its limit");
        }
        if self.delivered.windows(2).any(|pair| pair[0] >= pair[1]) {
            self.problems.push("data left out of its order");
        }
        if self.refusals().pressure > pressure_before {
            let late = self
                .first_wait
                .is_some_and(|first| now >= first + NETWORK_LATE.as_micros());
            if !late {
                self.problems
                    .push("a unit was refused for pressure before the bound");
            }
        }
        // The episode: from the first wait, not extended by returns; it
        // ends once the ring was walked with nothing waiting.
        if waiting == 0 {
            if !self.stopped && self.around.port.ring_len() == 0 {
                self.first_wait = None;
            }
        } else if self.first_wait.is_none() {
            self.first_wait = Some(now);
        }
        if waiting > 0 && !self.stopped {
            let bound = self
                .first_wait
                .map(|first| Instant::from_micros(first + NETWORK_LATE.as_micros()));
            match (self.owed, bound) {
                (Rest::Wake(Some(at)), Some(bound)) if at == bound => {}
                (Rest::Wake(Some(_)), _) => {
                    self.problems.push("the episode's bound moved");
                }
                _ => self
                    .problems
                    .push("a unit waits for the network and no bound is armed"),
            }
            let room = usize::from(LEND).saturating_sub(self.around.net.lent.len());
            if room > 0 || self.around.net.copy_room > 0 {
                self.problems
                    .push("a unit waits for a network with room to lend or to copy");
            }
        }
        // A unit is in one place: the ring, a place, or gone (copied, read,
        // refused, given back by the network).
        let refused = {
            let r = self.refusals();
            usize::from(r.link_held)
                + usize::from(r.pressure)
                + usize::from(r.unverified)
                + usize::from(r.undecapsulable)
                + usize::from(r.amsdu_too_long)
                + usize::from(r.place)
        };
        let copied = self
            .around
            .net
            .received
            .iter()
            .filter(|got| matches!(got, Got::Copied(_)))
            .count();
        let accounted = self.around.port.ring_len()
            + self.around.port.places_held()
            + copied
            + self.around.told.frames.len()
            + refused
            + self.returned;
        if !self.stopped && accounted != self.arrived.len() {
            self.problems.push("a unit is in two places or in none");
        }
        if self.stopped && accounted > self.arrived.len() {
            self.problems.push("a unit is in two places");
        }
        if self.stopped && action == Move::Stop && waiting > 0 {
            self.problems
                .push("the data stopped with units waiting for the network");
        }
        if self.stopped && waiting > 0 {
            self.problems
                .push("a stopping engine took a unit from the ring");
        }
        // Work left has a wake: a round, an instant, or the network's
        // return.
        if self.owed != Rest::Round && !self.stopped {
            let work = self.around.port.ring_len() > 0 || waiting > 0;
            let wake = matches!(self.owed, Rest::Wake(Some(_))) || !self.around.net.lent.is_empty();
            if work && !wake {
                self.problems.push("work is left and nothing wakes for it");
            }
            let before = self.key();
            let again = self.engine.received(&mut self.around.turn());
            if self.key() != before || again == Rest::Round {
                self.problems.push("work waits for a call nobody makes");
            }
        }
    }

    fn key(&mut self) -> String {
        std::format!(
            "{:?}|{:?}|{:?}|{}|{}|{}|{:?}|{}|{}|{}|{}",
            (
                self.left.units,
                self.left.returns,
                self.left.stops,
                self.left.frees
            ),
            self.arrived,
            self.delivered,
            self.returned,
            self.stopped,
            self.first_wait.is_some(),
            self.owed,
            self.around.net.copy_room,
            self.around.net.lent.len(),
            self.engine.key(),
            self.around.port.state_key(),
        )
    }
}

struct Visit {
    problems: Vec<&'static str>,
    moves: Vec<Move>,
    key: String,
}

fn replay(path: &[Move], bounds: Bounds) -> Visit {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut world = World::new(run, bounds);
        for (index, action) in path.iter().enumerate() {
            if index + 1 == path.len() {
                world.problems.clear();
            }
            world.apply(*action);
        }
        Visit {
            problems: core::mem::take(&mut world.problems),
            moves: world.moves(),
            key: world.key(),
        }
    })
}

/// What a search found.
pub(super) struct Found {
    pub nodes: usize,
    pub ways: usize,
    pub depth: usize,
    pub problems: Vec<(&'static str, String)>,
}

pub(super) fn search(bounds: Bounds) -> Found {
    let mut found = Found {
        nodes: 0,
        ways: 0,
        depth: 0,
        problems: Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    let mut stack: Vec<Vec<Move>> = std::vec![Vec::new()];
    while let Some(path) = stack.pop() {
        let visit = replay(&path, bounds);
        found.ways += 1;
        found.depth = found.depth.max(path.len());
        for problem in visit.problems {
            if !found.problems.iter().any(|(seen, _)| *seen == problem) {
                found.problems.push((problem, std::format!("{path:?}")));
            }
        }
        if !seen.insert(visit.key) {
            continue;
        }
        found.nodes += 1;
        if path.len() >= DEPTH {
            continue;
        }
        for action in visit.moves {
            let mut child = path.clone();
            child.push(action);
            stack.push(child);
        }
    }
    found
}

fn run(bounds: Bounds) {
    let start = Clock::now();
    let found = search(bounds);
    std::println!(
        "receive search {bounds:?}: {} states, {} ways, depth {}, {:?}",
        found.nodes,
        found.ways,
        found.depth,
        start.elapsed()
    );
    for (problem, path) in &found.problems {
        std::println!("PROBLEM {problem}\n    by {path}");
    }
    assert!(found.problems.is_empty());
}

/// The search at the model's bounds, in the ordinary test run.
#[test]
fn every_order_of_the_receive_sections_keeps_its_properties() {
    run(Bounds {
        units: 4,
        returns: 2,
        stops: 1,
        frees: 1,
    });
}

/// The search at the bounds the model's mutants run at. Named, not run by
/// default: its size and time are in the pull request.
#[test]
#[ignore = "the mutants' bounds: run by name"]
fn every_receive_order_at_the_mutants_bounds() {
    run(Bounds {
        units: 7,
        returns: 2,
        stops: 1,
        frees: 2,
    });
}
