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
    /// Frames that arrive, of the mode's list ([`PLAIN`], [`AGREED`]).
    pub units: usize,
    /// Lent units the network gives back.
    pub returns: usize,
    /// Stops of the data.
    pub stops: usize,
    /// Copies the network consumes, each giving its room back.
    pub frees: usize,
    /// Reorder gaps that time out.
    pub gaps: usize,
}

/// Whether the world's link has a receive agreement on TID 0.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(super) enum Agreement {
    None,
    /// From sequence number 0, a window of eight.
    Tid0,
}

/// The frames that may arrive without an agreement, the first `units` of
/// them, a kind at a time in their order: the list C1 searched, with an
/// A-MSDU (which the network cannot borrow) second.
const PLAIN: [(Kind, u16); 8] = [
    (Kind::Data, 0),
    (Kind::Amsdu, 0),
    (Kind::Data, 0),
    (Kind::Management, 0),
    (Kind::Data, 0),
    (Kind::Data, 0),
    (Kind::Management, 0),
    (Kind::Data, 0),
];

/// The frames that may arrive under an agreement, the first `units` of
/// them, in any order, each with its number: one before the first, an
/// A-MSDU, the first, a management frame, a retry of a number sent, then
/// a gap (the model's list, ordered so that each kind is within the
/// ordinary bounds).
const AGREED: [(Kind, u16); 8] = [
    (Kind::Data, 1),
    (Kind::Amsdu, 2),
    (Kind::Data, 0),
    (Kind::Management, 0),
    (Kind::Data, 1),
    (Kind::Data, 3),
    (Kind::Data, 5),
    (Kind::Management, 0),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum Kind {
    Data,
    /// A data frame carrying an A-MSDU of two MSDUs: copied, never lent.
    Amsdu,
    Management,
}

/// One step of the search.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Move {
    /// The frame of this index of the mode's list arrives: the ring's signal.
    Arrives(usize),
    /// A reorder gap times out.
    Gap,
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
    agreement: Agreement,
    /// Each arrived frame's index of its mode's list, by its id less one.
    arrived: Vec<usize>,
    /// Under an agreement, the number the world expects the reorder to let
    /// go next: its own arithmetic of what arrived, what went and what a
    /// gap gave up.
    start: u16,
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
    fn new(run: Running<'id, STATION_SINGLES, 0>, bounds: Bounds, agreement: Agreement) -> Self {
        let mut engine = StationEngine::<'_, 2>::new(4, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        around.net.lend_limit = LEND;
        around.net.copy_room = 0;
        // Two units a service, as the model's frontier.
        around.port.set_frontier(2);
        if agreement == Agreement::Tid0 {
            around.port.land(&receive::addba(0), receive::meta());
            let _ = engine.received(&mut around.turn());
            let _ = engine.round(&mut around.turn());
            linked(&mut engine, |peer| {
                if let Some(reply) = peer.rx_reply() {
                    reply.sent();
                }
            });
        }
        Self {
            engine,
            around,
            left: bounds,
            agreement,
            start: 0,
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

    /// The frames that may arrive now: under an agreement any not yet
    /// arrived, else the first of each kind not yet arrived (data frames
    /// without an agreement differ only in their order).
    fn sends(&self) -> Vec<usize> {
        let limit = self.arrived.len() + self.left.units;
        let kinds = self.kinds();
        let left = (0..limit.min(kinds.len())).filter(|index| !self.arrived.contains(index));
        match self.agreement {
            Agreement::Tid0 => left.collect(),
            Agreement::None => {
                let mut sends = Vec::new();
                for index in left {
                    if !sends
                        .iter()
                        .any(|seen: &usize| kinds[*seen].0 == kinds[index].0)
                    {
                        sends.push(index);
                    }
                }
                sends
            }
        }
    }

    /// The mode's list of frames.
    const fn kinds(&self) -> &'static [(Kind, u16); 8] {
        match self.agreement {
            Agreement::None => &PLAIN,
            Agreement::Tid0 => &AGREED,
        }
    }

    /// The numbers of the units the engine holds in a place and has not
    /// lent.
    fn held_units(&self) -> Vec<u16> {
        self.waiting_places()
            .iter()
            .filter_map(|place| {
                let bytes = self.around.port.place_bytes(*place)?;
                (bytes.len() > 24).then(|| u16::from_le_bytes([bytes[22], bytes[23]]) >> 4)
            })
            .collect()
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

    /// The places of the units that wait for the network, by the world's
    /// own count (see [`Self::waiting`]): a unit the reorder retains is not
    /// among them.
    fn waiting_run(&self) -> Vec<u8> {
        let places = self.waiting_places();
        match self.agreement {
            Agreement::None => places,
            Agreement::Tid0 => {
                let run = u16::try_from(self.waiting()).unwrap_or(u16::MAX);
                places
                    .into_iter()
                    .filter(|place| {
                        self.around.port.place_bytes(*place).is_some_and(|bytes| {
                            bytes.len() > 24
                                && (u16::from_le_bytes([bytes[22], bytes[23]]) >> 4)
                                    .wrapping_sub(self.start)
                                    < run
                        })
                    })
                    .collect()
            }
        }
    }

    /// The units that wait for the network, by the world's own count:
    /// without an agreement every unit held and not lent; under one, the
    /// run of numbers from the next the reorder lets go (released in order,
    /// not yet taken by the network).
    fn waiting(&self) -> usize {
        match self.agreement {
            Agreement::None => self.waiting_places().len(),
            Agreement::Tid0 => {
                let held = self.held_units();
                (0_u16..)
                    .take_while(|n| held.contains(&self.start.wrapping_add(*n)))
                    .count()
            }
        }
    }

    /// The units the reorder retains, by the world's own count: the held
    /// ones beyond the waiting run.
    fn retained(&self) -> usize {
        self.waiting_places().len().saturating_sub(self.waiting())
    }

    fn moves(&self) -> Vec<Move> {
        let mut moves = Vec::new();
        if self.left.units > 0 {
            moves.extend(self.sends().into_iter().map(Move::Arrives));
        }
        if self.left.gaps > 0 && self.retained() > 0 && !self.stopped {
            moves.push(Move::Gap);
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
        let held_before = self.held_units();
        let waiting_before = u16::try_from(self.waiting()).unwrap_or(u16::MAX);
        let delivered_before = self.delivered.len();
        let empty_before = held_before.is_empty() && self.around.net.lent.is_empty();
        let reorder_before = self.refusals().reorder;
        let waited = self.waiting_run();
        let services = self.around.port.counts().services;
        let rest = match action {
            Move::Arrives(index) => {
                self.left.units -= 1;
                self.arrived.push(index);
                let id = self.arrived.len() as u8;
                let (kind, sequence) = self.kinds()[index];
                let frame = match (kind, self.agreement) {
                    (Kind::Data, Agreement::Tid0) => receive::qos_data(sequence, id),
                    (Kind::Data, Agreement::None) => {
                        receive::data(u16::from(id), receive::IPV4, id)
                    }
                    (Kind::Amsdu, _) => receive::amsdu_numbered(sequence, &[id, id]),
                    (Kind::Management, _) => receive::beacon(u16::from(id)),
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
            Move::Gap => {
                self.left.gaps -= 1;
                // The gap gives the hole up: the reorder lets the first
                // retained run go.
                let waiting = u16::try_from(self.waiting()).unwrap_or(u16::MAX);
                if let Some(first) = self
                    .held_units()
                    .into_iter()
                    .filter(|n| n.wrapping_sub(self.start) >= waiting)
                    .min()
                {
                    self.start = first;
                }
                self.around.micros += POLICY.gap.as_micros();
                Some(self.engine.due(&mut self.around.turn()))
            }
            Move::Late => {
                let bound = self.first_wait.map_or(0, |first| first + LATE.as_micros());
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
        // With nothing held and nothing lent, a frame after a hole has its
        // place in the reorder, however many units went and came back.
        if let Move::Arrives(index) = action
            && self.agreement == Agreement::Tid0
            && empty_before
            && !self.stopped
            && AGREED[index].0 != Kind::Management
            && AGREED[index].1 > self.start
            && self.refusals().reorder != reorder_before
        {
            self.problems
                .push("a frame after a hole was refused with nothing held or lent");
        }
        if self.agreement == Agreement::Tid0 {
            self.advance_start(&held_before, waiting_before, delivered_before);
        }
        // While a unit waits for the network, the ring is not serviced: a
        // unit that waited before the step and still waits after it says
        // the round was held back.
        let still = self
            .waiting_run()
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

    /// The number the reorder lets go next, by the world's arithmetic: past
    /// each number delivered, and past each that waited and left without
    /// being delivered (refused for pressure).
    fn advance_start(&mut self, held_before: &[u16], waiting_before: u16, delivered_before: usize) {
        let numbers: Vec<u16> = self.delivered[delivered_before..]
            .iter()
            .filter_map(|id| self.arrived.get(usize::from(*id).checked_sub(1)?))
            .map(|index| AGREED[*index].1)
            .collect();
        let held_now = self.held_units();
        let start = self.start;
        let left: Vec<u16> = held_before
            .iter()
            .copied()
            .filter(|n| n.wrapping_sub(start) < waiting_before && !held_now.contains(n))
            .collect();
        for number in numbers.into_iter().chain(left) {
            if number >= self.start {
                self.start = number.wrapping_add(1);
            }
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
        // Data leaves in order: under an agreement its sequence numbers,
        // else its arrival.
        let order: Vec<u16> = match self.agreement {
            Agreement::Tid0 => self
                .delivered
                .iter()
                .filter_map(|id| self.arrived.get(usize::from(*id).checked_sub(1)?))
                .map(|index| AGREED[*index].1)
                .collect(),
            Agreement::None => self.delivered.iter().map(|id| u16::from(*id)).collect(),
        };
        // An A-MSDU's MSDUs go one after another under one unit's id.
        let mut order = order;
        order.dedup();
        if order.windows(2).any(|pair| pair[0] >= pair[1]) {
            self.problems.push("data left out of its order");
        }
        if self.agreement == Agreement::Tid0 && !self.stopped && waiting == 0 {
            let held = self.around.port.places_held();
            if held >= oer_protocols_ieee80211_testing_model::MODEL_PLACES {
                self.problems
                    .push("the frame that closes the hole has no place");
            }
        }
        if self.stopped && self.retained() > 0 {
            self.problems
                .push("the data stopped with the reorder's units held");
        }
        // A retained unit that follows in order is released at once: with
        // nothing waiting for the network and nothing refused for pressure,
        // no unit the engine holds carries the next number.
        if self.agreement == Agreement::Tid0
            && !self.stopped
            && waiting == 0
            && self.refusals().pressure == 0
        {
            let next = order.last().map_or(0, |last| last + 1);
            let held = self.waiting_places();
            let follows = held.iter().any(|place| {
                self.around.port.place_bytes(*place).is_some_and(|bytes| {
                    bytes.len() > 24 && u16::from_le_bytes([bytes[22], bytes[23]]) >> 4 == next
                })
            });
            if follows {
                self.problems
                    .push("a held unit follows in order and was not released");
            }
        }
        // A retry of a frame the reorder holds goes back: no two units the
        // engine holds carry one number.
        if self.agreement == Agreement::Tid0 {
            let mut numbers: Vec<u16> = self
                .waiting_places()
                .iter()
                .filter_map(|place| {
                    let bytes = self.around.port.place_bytes(*place)?;
                    (bytes.len() > 24).then(|| u16::from_le_bytes([bytes[22], bytes[23]]) >> 4)
                })
                .collect();
            let all = numbers.len();
            numbers.sort_unstable();
            numbers.dedup();
            if numbers.len() != all {
                self.problems
                    .push("a retry of a frame already held is held too");
            }
        }
        if self.refusals().pressure > pressure_before {
            let late = self
                .first_wait
                .is_some_and(|first| now >= first + LATE.as_micros());
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
                .map(|first| Instant::from_micros(first + LATE.as_micros()));
            match (self.owed, bound) {
                (Rest::Wake(Some(at)), Some(bound)) if at == bound => {}
                (Rest::Wake(Some(_)), _) => {
                    self.problems.push("the episode's bound moved");
                }
                _ => self
                    .problems
                    .push("a unit waits for the network and no bound is armed"),
            }
            // The first unit that waits: without an agreement the earliest
            // held (its tag is its id), under one the held unit of the
            // number the reorder lets go next. A single frame needs room to
            // lend or one copy, an A-MSDU a copy of each of its two MSDUs.
            let lend = usize::from(LEND).saturating_sub(self.around.net.lent.len());
            let start = self.start;
            let agreement = self.agreement;
            let first = self
                .waiting_places()
                .iter()
                .filter_map(|place| {
                    let bytes = self.around.port.place_bytes(*place)?;
                    let number =
                        (bytes.len() > 24).then(|| u16::from_le_bytes([bytes[22], bytes[23]]) >> 4);
                    let id = bytes.last().copied()?;
                    match agreement {
                        Agreement::Tid0 if number != Some(start) => None,
                        _ => Some(id),
                    }
                })
                .min()
                .and_then(|id| self.arrived.get(usize::from(id).checked_sub(1)?))
                .map(|index| self.kinds()[*index].0);
            let copies = usize::from(self.around.net.copy_room);
            let room = match first {
                Some(Kind::Amsdu) => copies >= 2,
                _ => lend > 0 || copies > 0,
            };
            if room {
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
                + usize::from(r.reorder)
                + usize::from(r.place)
        };
        // Units copied, an A-MSDU's MSDUs (each with its unit's tag) once.
        let mut copied: Vec<u8> = self
            .around
            .net
            .received
            .iter()
            .filter_map(|got| match got {
                Got::Copied(frame) => frame.last().copied(),
                Got::Lent(_) => None,
            })
            .collect();
        copied.sort_unstable();
        copied.dedup();
        let copied = copied.len();
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
                self.left.frees,
                self.left.gaps,
                self.engine.rx_held(),
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

fn replay(path: &[Move], bounds: Bounds, agreement: Agreement) -> Visit {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut world = World::new(run, bounds, agreement);
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

pub(super) fn search(bounds: Bounds, agreement: Agreement) -> Found {
    let mut found = Found {
        nodes: 0,
        ways: 0,
        depth: 0,
        problems: Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    let mut stack: Vec<Vec<Move>> = std::vec![Vec::new()];
    while let Some(path) = stack.pop() {
        let visit = replay(&path, bounds, agreement);
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

fn run(bounds: Bounds, agreement: Agreement) {
    let start = Clock::now();
    let found = search(bounds, agreement);
    std::println!(
        "receive search {bounds:?} {agreement:?}: {} states, {} ways, depth {}, {:?}",
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
    run(
        Bounds {
            units: 4,
            returns: 2,
            stops: 1,
            frees: 1,
            gaps: 0,
        },
        Agreement::None,
    );
}

/// The same under a receive agreement: the reorder's order, its gap, the
/// place it keeps free, in the ordinary test run.
#[test]
fn every_order_under_a_receive_agreement_keeps_its_properties() {
    run(
        Bounds {
            units: 5,
            returns: 2,
            stops: 1,
            frees: 1,
            gaps: 1,
        },
        Agreement::Tid0,
    );
}

/// The search at the bounds the model's mutants run at. Named, not run by
/// default: its size and time are in the pull request.
#[test]
#[ignore = "the mutants' bounds: run by name"]
fn every_receive_order_at_the_mutants_bounds() {
    run(
        Bounds {
            units: 7,
            returns: 2,
            stops: 1,
            frees: 2,
            gaps: 0,
        },
        Agreement::None,
    );
}

/// The agreement's search at the model's mutants' bounds. Named.
#[test]
#[ignore = "the mutants' bounds: run by name"]
fn every_order_under_a_receive_agreement_at_the_mutants_bounds() {
    run(
        Bounds {
            units: 6,
            returns: 2,
            stops: 1,
            frees: 1,
            gaps: 1,
        },
        Agreement::Tid0,
    );
}
