//! Every order of a station engine's receive sections over the model port,
//! as `design/engine-model-rx.py` searches its model: the ring's signal
//! with a data frame, an A-MSDU, EAPOL or a management frame arrived, the network's return of a lent unit, its copy room
//! changed, the reorder's gap, the head's bound, the controller's hold of
//! the link and its release, the data's stop, a round owed.
//!
//! The network lends one unit at most and copies one frame a round while
//! its copy room is open; the model port has eight places. The world keeps
//! its own record: what arrived, what the network got and gave back, what
//! the controller read, the moment each unit began to wait. Properties are
//! read through the engine's public API and what the model port and the
//! test's network show; each is named by the model's number (`P1` …
//! `P14`), and the model's properties without a twin here are named in
//! the pull request. States are merged by their whole key, the engine's
//! and the model port's included. The waiting units' moments enter it as
//! their order with its ties, and the agreements' gap deadline as its place
//! in that order: the values go, the order stays. This loses no world the
//! rule tells apart: the rule reads a moment only against a deadline (a
//! head is late, a gap is due), the search's clock moves only to a
//! deadline (`Late`, `Gap`) or by a step's microsecond, which changes no
//! deadline's order, and between steps no unit is late (P8).

use super::*;
use std::string::String;
use std::time::Instant as Clock;

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
    /// Holds of the link by its controller (each may be released).
    pub holds: usize,
    /// The units the network may hold at once (the model's loan is one).
    pub loans: u8,
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
/// A-MSDU (which the network cannot borrow) second and EAPOL fifth.
const PLAIN: [(Kind, u16); 8] = [
    (Kind::Data, 0),
    (Kind::Amsdu, 0),
    (Kind::Data, 0),
    (Kind::Management, 0),
    (Kind::Eapol, 0),
    (Kind::Data, 0),
    (Kind::Management, 0),
    (Kind::Data, 0),
];

/// The frames that may arrive under an agreement, the first `units` of
/// them, in any order, each with its number: one before the first, an
/// A-MSDU, the first, a management frame, a fourth number (sent only once
/// the first came, so that it lies inside the small port's window of three
/// and the world's arithmetic needs no window's move), then retries (the model's list, ordered so that each
/// kind is within the ordinary bounds).
const AGREED: [(Kind, u16); 8] = [
    (Kind::Data, 1),
    (Kind::Amsdu, 2),
    (Kind::Data, 0),
    (Kind::Management, 0),
    (Kind::Data, 3),
    (Kind::Data, 1),
    (Kind::Data, 2),
    (Kind::Management, 0),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum Kind {
    Data,
    /// A data frame carrying an A-MSDU of two MSDUs: copied, never lent.
    Amsdu,
    /// EAPOL of the linked peer: the controller's.
    Eapol,
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
    /// The head's bound comes: the clock moves to the oldest waiting
    /// unit's moment and `late`.
    Late,
    /// The controller holds the link.
    Hold,
    /// The controller lets it go again.
    Release,
    /// The data's stop.
    Stop,
    /// A round owed.
    Round,
}

/// The link as the world's controller left it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Link {
    Carrying,
    Held,
}

/// The world over a model port of `P` staging places, the receive window
/// `W`.
struct World<'id, const P: usize, const W: usize> {
    engine: StationEngineOn<'id, 2, P, W>,
    around: Around<'id, STATION_SINGLES, 0, P>,
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
    /// The deliveries the last check saw.
    delivered_seen: usize,
    /// Numbers the reorder took and passed undelivered (a frame refused for
    /// no place to wait), by the world's arithmetic.
    skipped: Vec<u16>,
    /// The stop asked.
    stopped: bool,
    /// Whether the controller holds the link.
    link: Link,
    /// The world's record of each unit waiting for the network: its id and
    /// the micros it began to wait, kept apart from the engine's.
    waits: Vec<(u8, u64)>,
    owed: Rest,
    problems: Vec<&'static str>,
}

impl<'id, const P: usize, const W: usize> World<'id, P, W> {
    fn new(run: Running<'id, STATION_SINGLES, 0, P>, bounds: Bounds, agreement: Agreement) -> Self {
        let mut engine = StationEngineOn::<'_, 2, P, W>::new(4, POLICY);
        let mut around = station(run, &mut engine, 2, HOUR);
        around.net.lend_limit = bounds.loans;
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
            delivered_seen: 0,
            skipped: Vec::new(),
            stopped: false,
            link: Link::Carrying,
            waits: Vec::new(),
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
            // Number 3 only once number 0 came: inside the window.
            Agreement::Tid0 => left
                .filter(|index| {
                    kinds[*index].1 != 3
                        || self
                            .arrived
                            .iter()
                            .any(|seen| kinds[*seen] == (Kind::Data, 0))
                })
                .collect(),
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
                let run = self.span();
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
                let span = self.span();
                self.held_units()
                    .into_iter()
                    .filter(|n| n.wrapping_sub(self.start) < span)
                    .count()
            }
        }
    }

    /// The numbers from the next the reorder lets go that it let go: held
    /// units in order, through the numbers it passed undelivered.
    fn span(&self) -> u16 {
        let held = self.held_units();
        (0_u16..)
            .take_while(|n| {
                let number = self.start.wrapping_add(*n);
                held.contains(&number) || self.skipped.contains(&number)
            })
            .count() as u16
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
        if self.left.holds > 0 && self.link == Link::Carrying && !self.stopped {
            moves.push(Move::Hold);
        }
        if self.link == Link::Held && !self.stopped {
            moves.push(Move::Release);
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
        let refused_before = self.refusals();
        let engine_refused_before = self.engine.rx_refusals();
        let waits_before = self.waits.clone();
        let link_before = self.link;
        let retained_before = self.retained();
        let held_before = self.held_units();
        let waiting_before = u16::try_from(self.waiting()).unwrap_or(u16::MAX);
        let delivered_before = self.delivered.len();
        let empty_before = held_before.is_empty() && self.around.net.lent.is_empty();
        let reorder_before = self.refusals().reorder;
        let waited = self.waiting_run();
        let services = self.around.port.counts().services;
        let ring_before = self.around.port.ring_len();
        let places_before = self.around.port.places_held();
        let rest = match action {
            Move::Arrives(index) => {
                self.left.units -= 1;
                self.arrived.push(index);
                let id = self.arrived.len() as u8;
                let (kind, sequence) = self.kinds()[index];
                let eapol = oer_protocols_ieee80211_wire::ethertype::ETHER_TYPE_EAPOL;
                let frame = match (kind, self.agreement) {
                    (Kind::Data, Agreement::Tid0) => receive::qos_data(sequence, id),
                    (Kind::Data, Agreement::None) => {
                        receive::data(u16::from(id), receive::IPV4, id)
                    }
                    (Kind::Eapol, _) => receive::data(u16::from(id), eapol, id),
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
                // The clock moves to the head's bound, by the world's record.
                let bound = self
                    .waits
                    .iter()
                    .map(|(_, since)| since + LATE.as_micros())
                    .min()
                    .unwrap_or(0);
                if bound > self.around.micros + 1 {
                    self.around.micros = bound - 1;
                }
                Some(self.engine.due(&mut self.around.turn()))
            }
            Move::Hold => {
                self.left.holds -= 1;
                self.link = Link::Held;
                let around = &mut self.around;
                let mut rest = Rest::Wake(None);
                linked(&mut self.engine, |peer| {
                    rest = peer.hold(&mut around.turn())
                });
                Some(rest)
            }
            Move::Release => {
                self.link = Link::Carrying;
                let around = &mut self.around;
                let mut rest = Rest::Wake(None);
                linked(&mut self.engine, |peer| {
                    rest = peer.release(&mut around.turn())
                });
                Some(rest)
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
            // A frame in order refused as it came for no place to wait: the
            // reorder took its number, which passes undelivered (a held
            // link's data is refused before the reorder and takes none).
            if let Move::Arrives(index) = action
                && !self.stopped
                && self.refusals().no_place_to_wait > refused_before.no_place_to_wait
            {
                let id = u8::try_from(self.arrived.len()).unwrap_or(u8::MAX);
                let (kind, number) = AGREED[index];
                let placed = self.around.port.held_places().into_iter().any(|place| {
                    self.around
                        .port
                        .place_bytes(place)
                        .is_some_and(|bytes| bytes.last() == Some(&id))
                });
                if matches!(kind, Kind::Data | Kind::Amsdu)
                    && !placed
                    && !self.delivered.contains(&id)
                    && number.wrapping_sub(self.start) < 0x800
                {
                    self.skipped.push(number);
                }
            }
            self.advance_start(&held_before, waiting_before, delivered_before);
        }
        let _ = (waited, services);
        // P5: a round that finds a free place services the ring: a unit
        // the ring held at the step's start, or the one that landed, left
        // it.
        let landed = usize::from(matches!(action, Move::Arrives(_)));
        let ring_in = ring_before + landed;
        if !self.stopped
            && action != Move::Stop
            && ring_in > 0
            && places_before < P
            && self.around.port.ring_len() >= ring_in
        {
            self.problems
                .push("P5 a round found a free place and did not service the ring");
        }
        self.record_waits();
        // P9: data that arrives while the link is held is refused, never
        // kept for the reorder.
        if link_before == Link::Held
            && self.link == Link::Held
            && matches!(action, Move::Arrives(index) if matches!(self.kinds()[index].0, Kind::Data | Kind::Amsdu))
            && self.retained() > retained_before
        {
            self.problems
                .push("P9 data of a held link was kept for the reorder");
        }
        self.check(
            action,
            refused_before,
            &waits_before,
            link_before,
            places_before,
            engine_refused_before,
        );
    }

    /// The world's record of the waiting units: one that began to wait in
    /// this step takes the step's micros; one that left is dropped.
    fn record_waits(&mut self) {
        let now = self.around.micros;
        let ids: Vec<u8> = self
            .waiting_run()
            .iter()
            .filter_map(|place| self.around.port.place_bytes(*place)?.last().copied())
            .collect();
        self.waits.retain(|(id, _)| ids.contains(id));
        for id in ids {
            if !self.waits.iter().any(|(seen, _)| *seen == id) {
                self.waits.push((id, now));
            }
        }
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
        // A number passed undelivered is passed by the world too.
        while self.skipped.contains(&self.start) && !self.held_units().contains(&self.start) {
            self.start = self.start.wrapping_add(1);
        }
        let start = self.start;
        self.skipped
            .retain(|number| number.wrapping_sub(start) < 0x800);
    }

    /// The world's record of what the network and the controller got.
    fn read(&mut self, action: Move) {
        let received = &self.around.net.received[self.received_read..];
        // Nothing is delivered after the stop, its own round included (P13).
        if self.stopped && !received.is_empty() {
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

    fn check(
        &mut self,
        action: Move,
        before: RxRefusals,
        waits_before: &[(u8, u64)],
        link_before: Link,
        places_before: usize,
        engine_refused_before: RxRefusals,
    ) {
        let now = self.around.micros;
        let waiting = self.waiting();
        let refused = self.refusals();
        if self.around.net.lent.len() > usize::from(self.left.loans) {
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
            self.problems.push("P3 data left out of its order");
        }
        let places = P;
        // P6 (and P10's place): while the data flows a place is free when a
        // round closes, the one the control plane and the hole's frame pass.
        if !self.stopped && self.around.port.places_held() >= places {
            self.problems.push("P6 a round closed with no place free");
        }
        // P5: the ring is serviced in every round that finds a free place.
        if !self.stopped
            && self.owed != Rest::Round
            && self.around.port.ring_len() > 0
            && self.around.port.places_held() < places
        {
            self.problems
                .push("P5 a round found a free place and did not service the ring");
        }
        // P4: every management frame and EAPOL that left the ring while the
        // data flows was read in its round (without an agreement EAPOL is
        // read at once; under one it takes its number first).
        if !self.stopped {
            let staged = self
                .arrived
                .len()
                .saturating_sub(self.around.port.ring_len());
            let controls = self.arrived[..staged]
                .iter()
                .filter(|index| {
                    matches!(
                        (self.kinds()[**index].0, self.agreement),
                        (Kind::Management, _) | (Kind::Eapol, Agreement::None)
                    )
                })
                .count();
            if self.around.told.frames.len() < controls {
                self.problems
                    .push("P4 a management frame or EAPOL left unread while the data flows");
            }
        }
        // P7: each refusal has its reason.
        let late = usize::from(refused.late.saturating_sub(before.late));
        if late > 0 {
            let due = waits_before
                .iter()
                .filter(|(id, since)| {
                    since + LATE.as_micros() <= now && !self.waits.iter().any(|(w, _)| w == id)
                })
                .count();
            if due < late {
                self.problems
                    .push("P7 a unit was refused late before it waited LATE from its own moment");
            }
        }
        if refused.link_held > before.link_held
            && link_before == Link::Carrying
            && self.link == Link::Carrying
        {
            self.problems
                .push("P7 a unit was refused for a hold that was not");
        }
        // A newcomer is refused for no place only where the places the
        // step began with and the newcomer leave none free.
        if refused.no_place_to_wait > before.no_place_to_wait && places_before + 1 < places {
            self.problems
                .push("P7 a unit was refused for no place to wait with places free");
        }
        // P8: no unit that waited `late` from its own moment still waits.
        if !self.stopped
            && self
                .waits
                .iter()
                .any(|(_, since)| since + LATE.as_micros() <= now)
        {
            self.problems
                .push("P8 a unit that waited LATE from its own moment still waits");
        }
        // P9: nothing is delivered while the link is held.
        if link_before == Link::Held
            && self.link == Link::Held
            && self.delivered.len() > self.delivered_seen
        {
            self.problems
                .push("P9 data was delivered while the link is held");
        }
        self.delivered_seen = self.delivered.len();
        if self.stopped && self.retained() > 0 {
            self.problems
                .push("P13 the data stopped with the reorder's units held");
        }
        // P12: a retained unit that follows in order is released at once:
        // with nothing waiting for the network and nothing refused, no unit
        // the engine holds carries the next number.
        if self.agreement == Agreement::Tid0
            && !self.stopped
            && waiting == 0
            && refused.no_place_to_wait == 0
            && refused.late == 0
            && refused.link_held == 0
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
                    .push("P12 a held unit follows in order and was not released");
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
        if waiting > 0 && !self.stopped {
            // P14: the head's bound is its own moment and `late`; a reorder
            // that retains may wake sooner, by its gap.
            let bound = self
                .waits
                .iter()
                .map(|(_, since)| Instant::from_micros(since + LATE.as_micros()))
                .min();
            match (self.owed, bound) {
                (Rest::Wake(Some(at)), Some(bound))
                    if at == bound || (self.retained() > 0 && at < bound) => {}
                (Rest::Round, _) => {}
                (Rest::Wake(Some(_)), _) => {
                    self.problems.push("P14 the head's bound is not its own");
                }
                _ => self
                    .problems
                    .push("P14 a unit waits for the network and no bound is armed"),
            }
            // The first unit that waits: without an agreement the earliest
            // held (its tag is its id), under one the held unit of the
            // number the reorder lets go next. A single frame needs room to
            // lend or one copy, an A-MSDU a copy of each of its two MSDUs.
            let lend = usize::from(self.left.loans).saturating_sub(self.around.net.lent.len());
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
            let r = refused;
            usize::from(r.link_held)
                + usize::from(r.no_place_to_wait)
                + usize::from(r.late)
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
            self.problems.push("P1 a unit is in two places or in none");
        }
        if self.stopped && accounted > self.arrived.len() {
            self.problems.push("a unit is in two places");
        }
        // P13: a stop refuses nothing: what waited goes back unread.
        if action == Move::Stop && self.engine.rx_refusals() != engine_refused_before {
            self.problems
                .push("P13 the stop refused what waited instead of giving it back");
        }
        if self.stopped && action == Move::Stop && waiting > 0 {
            self.problems
                .push("P13 the data stopped with units waiting for the network");
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
                self.problems
                    .push("P14 work is left and nothing wakes for it");
            }
            let before = self.key();
            let again = self.engine.received(&mut self.around.turn());
            if self.key() != before || again == Rest::Round {
                self.problems.push("P14 work waits for a call nobody makes");
            }
        }
    }

    /// The world's waiting units for the key: each id with the rank of its
    /// moment among the waiting, equal moments equal ranks (the values go,
    /// the order stays).
    fn relative_waits(&self) -> Vec<(u8, usize)> {
        let mut moments: Vec<u64> = self.waits.iter().map(|(_, since)| *since).collect();
        moments.sort_unstable();
        moments.dedup();
        self.waits
            .iter()
            .map(|(id, since)| (*id, moments.iter().position(|at| at == since).unwrap_or(0)))
            .collect()
    }

    fn key(&mut self) -> String {
        std::format!(
            "{:?}|{:?}|{:?}|{}|{}|{:?}|{:?}|{}|{}|{}|{}",
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
            (
                self.link,
                self.left.holds,
                self.relative_waits(),
                &self.skipped
            ),
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

fn replay<const P: usize, const W: usize>(
    path: &[Move],
    bounds: Bounds,
    agreement: Agreement,
) -> Visit {
    crate::model::running_on::<STATION_SINGLES, 0, P, _>(|run| {
        let mut world = World::<P, W>::new(run, bounds, agreement);
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

pub(super) fn search<const P: usize, const W: usize>(
    bounds: Bounds,
    agreement: Agreement,
    ceiling: usize,
) -> Found {
    let mut found = Found {
        nodes: 0,
        ways: 0,
        depth: 0,
        problems: Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    let mut stack: Vec<Vec<Move>> = std::vec![Vec::new()];
    while let Some(path) = stack.pop() {
        let visit = replay::<P, W>(&path, bounds, agreement);
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
        // A ceiling against an explosion, not a pinned figure.
        if found.nodes > ceiling {
            found.problems.push((
                "the search passed its ceiling of nodes",
                std::format!("{} nodes over a ceiling of {ceiling}", found.nodes),
            ));
            break;
        }
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

fn run<const P: usize, const W: usize>(bounds: Bounds, agreement: Agreement, ceiling: usize) {
    let start = Clock::now();
    let found = search::<P, W>(bounds, agreement, ceiling);
    std::println!(
        "receive search, {P} places, window {W}, {bounds:?} {agreement:?}: {} states, {} ways, depth {}, {:?}",
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

/// The search's main configuration takes the model's numbers
/// (`design/engine-model-rx.py`): four staging places, a loan of one, a
/// window of three, so that the last free place lies three units deep. The
/// model port's eight places with the tests' window of seven stay as one
/// ordinary bound, for what only a larger port shows.
const SMALL_PLACES: usize = 4;
const SMALL_WINDOW: usize = 3;
const LARGE_PLACES: usize = oer_protocols_ieee80211_testing_model::MODEL_PLACES;
const LARGE_WINDOW: usize = MODEL_WINDOW;

/// Each bound's ceiling of nodes: about three times what it takes today
/// (7 052, 26 209 and, over eight places, 7 052), a ceiling against an
/// explosion, never a pinned figure. The pressure rule's own orders are
/// searched by its core (`receive::pressure`); these are the round's.
const SMALL_CEILING: usize = 25_000;
const SMALL_AGREED_CEILING: usize = 80_000;
const LARGE_CEILING: usize = 25_000;

/// The small port at its ordinary bounds, without an agreement.
#[test]
fn every_order_of_the_receive_sections_keeps_its_properties() {
    run::<SMALL_PLACES, SMALL_WINDOW>(
        Bounds {
            units: 4,
            returns: 2,
            stops: 1,
            frees: 1,
            gaps: 0,
            holds: 1,
            loans: 1,
        },
        Agreement::None,
        SMALL_CEILING,
    );
}

/// The same under a receive agreement: the reorder's order, its gap, the
/// place it keeps free, a hold.
#[test]
fn every_order_under_a_receive_agreement_keeps_its_properties() {
    run::<SMALL_PLACES, SMALL_WINDOW>(
        Bounds {
            units: 4,
            returns: 2,
            stops: 1,
            frees: 1,
            gaps: 1,
            holds: 1,
            loans: 1,
        },
        Agreement::Tid0,
        SMALL_AGREED_CEILING,
    );
}

/// The model port's eight places, without an agreement: what only a larger
/// port shows.
#[test]
fn every_order_over_eight_places_keeps_its_properties() {
    run::<LARGE_PLACES, LARGE_WINDOW>(
        Bounds {
            units: 4,
            returns: 2,
            stops: 1,
            frees: 1,
            gaps: 0,
            holds: 1,
            loans: 1,
        },
        Agreement::None,
        LARGE_CEILING,
    );
}
