//! Every order of a station engine's sections over the model port, as
//! `design/engine-model.py` searches its model, at its bounds: two TIDs of
//! one access category, a window of two, three subframes an aggregate, a
//! pool of three, a budget of two, two attempts a frame; three MSDUs
//! offered, one loss, four requests.
//!
//! The engine holds rights, which are not copied, so a state is not kept:
//! each is reached again by replaying its path from a fresh installation.
//! States are merged by their whole key: the world's, the engine's private
//! fields (read in this crate's tests) and the model port's state. Only the
//! book's admission instants are left out: no lifetime (an hour) expires
//! within a path. Every step from a state is checked, also one into a state
//! seen before; a seen state is not explored again. The properties are read
//! through the engine's public API and the model port's.
//!
//! A publication is the port's choice of prefix: a step that published an
//! aggregate of `k` frames is explored again with the port admitting each
//! shorter prefix, one to `k - 1`.

use super::*;
use oer_protocols_ieee80211_contracts_port::{ArenaSide, GateClosed};
use oer_protocols_ieee80211_testing_model::ModelRights;
use oer_protocols_ieee80211_testing_model::StationModel;
use oer_protocols_ieee80211_testing_model::Went;
use std::string::String;
use std::time::Instant as Clock;

const TIDS: [WmmUserPriority; 2] = [WmmUserPriority::UP0, WmmUserPriority::UP3];
const WINDOW: u16 = 2;
const CAPACITY: u16 = 3;
const POOL: usize = 3;
const BUDGET: u8 = 2;
const ATTEMPTS: u8 = 2;
/// The longest path: every step spends a bound, ends an arena or answers
/// a round owed, so a longer one is a section that keeps owing work.
const DEPTH: usize = 40;

/// What the search may still do.
#[derive(Clone, Copy, Debug)]
pub(super) struct Bounds {
    pub offers: u32,
    pub losses: u8,
    pub requests: u8,
}

/// The answer the air gives the arena on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Answer {
    /// Every frame that went is acknowledged.
    Ack,
    /// None is.
    NoAck,
    /// The first only.
    First,
    /// All but the first.
    AllButFirst,
}

/// A request of the controller or of the installation machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ask {
    Link,
    Hold,
    Release,
    Unlink,
    Agree(usize),
    EndAgreement(usize),
    Sequence(usize),
    Named,
    CloseGate,
    OpenGate,
    Flows,
    StopData,
    /// The link's key changes (its installation, or a rekey).
    ChangeKey,
}

/// The named TID's place as its view shows it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Naming {
    Unnamed,
    Draining(usize),
    Named(usize),
}

/// The search's index of `tid`.
fn tid_index(tid: WmmUserPriority) -> usize {
    TIDS.iter()
        .position(|known| *known == tid)
        .unwrap_or(TIDS.len())
}

/// One step of the search.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Move {
    /// The network offers an MSDU of a TID.
    Offer(usize),
    /// The arena on the air ends.
    Ended(Answer),
    /// A request.
    Asked(Ask),
    /// The pump's turn: a round owed, or the command in progress polled.
    Turn,
    /// The machine asks the data's stop again.
    StopAgain,
}

#[derive(Clone, Copy, Debug)]
struct Step {
    action: Move,
    /// The most frames the step's first publication sends.
    prefix: Option<u8>,
}

/// The stop's last answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stop {
    NotAsked,
    Going,
    Stopped,
}

/// The world before a step.
struct Before {
    published: usize,
    keying: Awaited,
    arenas: Vec<u32>,
    /// The MSDUs the arenas keep for a retry, the air quiet.
    kept: Vec<u32>,
    /// The MSDU of a single MPDU on the air.
    single: Option<u32>,
    /// The named TID.
    hold: Option<usize>,
}

/// Whether the world asked for a command and was not yet told its end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Awaited {
    No,
    Yes,
}

/// The link's key as the world was last told.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KeyTold {
    Empty,
    Installed,
}

/// The engine and everything around it.
struct World<'id> {
    engine: StationEngine<'id, POOL>,
    around: Around<'id, StationModel<STATION_SINGLES>>,
    /// The data's rights, the machine's while the data is stopped.
    stopped: Option<ModelRights<'id, StationModel<STATION_SINGLES>>>,
    /// The controller's, while closed.
    gate: Option<GateClosed<'id>>,
    left: Bounds,
    offered: u32,
    /// What the last move owed.
    owed: Rest,
    stop: Stop,
    /// A close asked (or a stop with the gate lent), no `gate_closed` yet.
    closing: Awaited,
    /// A key change asked (or a stop with a key installed), no `key_ended`
    /// yet.
    keying: Awaited,
    key: KeyTold,
    /// The keys' ends read so far.
    keys_told: usize,
    /// The link generation: one more at each link.
    generation: u32,
    /// The generation each offered MSDU was offered in, by its id.
    born: Vec<u32>,
    /// The times each MSDU's frame went on the air, by its id.
    aired: Vec<u8>,
    agreed: [bool; 2],
    hold: Option<usize>,
    /// Each offered MSDU's TID.
    tids: Vec<usize>,
    /// The link generation each publication went in.
    /// The publications the order's check read.
    checked: usize,
    /// Where this link began: the publications and the homecomings before
    /// it are another link's.
    link_from: (usize, usize),
    /// Each TID's next first transmission's number, this link.
    next: [Option<u16>; 2],
    /// The numbers told for named TIDs, by TID, this generation.
    named: Vec<(usize, u16)>,
    /// A told number no first transmission of its TID carried yet.
    carried: [Option<u16>; 2],
    problems: Vec<&'static str>,
}

impl<'id> World<'id> {
    fn new(run: Running<'id, StationModel<STATION_SINGLES>>, bounds: Bounds) -> Self {
        let Running {
            port,
            data: rights,
            gate,
        } = run;
        Self {
            engine: StationEngine::new(BUDGET, POLICY),
            around: Around::new(port),
            stopped: Some(rights),
            gate: Some(gate),
            left: bounds,
            offered: 0,
            owed: Rest::Wake(None),
            stop: Stop::NotAsked,
            agreed: [false; 2],
            hold: None,
            tids: Vec::new(),
            checked: 0,
            link_from: (0, 0),
            closing: Awaited::No,
            keying: Awaited::No,
            key: KeyTold::Empty,
            keys_told: 0,
            generation: 0,
            born: Vec::new(),
            aired: Vec::new(),
            next: [None; 2],
            named: Vec::new(),
            carried: [None; 2],
            problems: Vec::new(),
        }
    }

    fn flowing(&mut self) -> bool {
        matches!(self.engine.data(), DataPlace::Flowing(_))
    }

    fn linked(&mut self) -> bool {
        match self.engine.data() {
            DataPlace::Flowing(mut flowing) => {
                matches!(flowing.peer(ACCESS_POINT), PeerPlace::Linked(_))
            }
            _ => false,
        }
    }

    fn held(&mut self) -> bool {
        match self.engine.data() {
            DataPlace::Flowing(mut flowing) => match flowing.peer(ACCESS_POINT) {
                PeerPlace::Linked(mut linked) => linked.is_held(),
                PeerPlace::Free(_) => false,
            },
            _ => false,
        }
    }

    /// The named TID's place, by the view: none named, its number not told,
    /// or told.
    fn naming(&mut self) -> Option<Naming> {
        match self.engine.data() {
            DataPlace::Flowing(mut flowing) => match flowing.peer(ACCESS_POINT) {
                PeerPlace::Linked(mut linked) => Some(match linked.naming() {
                    NamePlace::Unnamed(_) => Naming::Unnamed,
                    NamePlace::Draining(draining) => Naming::Draining(tid_index(draining.tid())),
                    NamePlace::Named(named) => Naming::Named(tid_index(named.tid())),
                }),
                PeerPlace::Free(_) => None,
            },
            _ => None,
        }
    }

    fn gate_open(&mut self) -> bool {
        match self.engine.data() {
            DataPlace::Flowing(mut flowing) => matches!(flowing.gate(), GatePlace::Open(_)),
            _ => false,
        }
    }

    /// Whether the gate is open or its closing runs.
    fn gate_not_closed(&mut self) -> bool {
        match self.engine.data() {
            DataPlace::Flowing(mut flowing) => !matches!(flowing.gate(), GatePlace::Shut(_)),
            DataPlace::Stopping(_) => true,
            DataPlace::Stopped(_) => false,
        }
    }

    /// Whether the key's place admits a change.
    fn key_idle(&mut self) -> bool {
        match self.engine.data() {
            DataPlace::Flowing(mut flowing) => match flowing.peer(ACCESS_POINT) {
                PeerPlace::Linked(mut linked) => matches!(linked.key(), KeyPlace::Idle(_)),
                PeerPlace::Free(_) => false,
            },
            _ => false,
        }
    }

    fn on_air(&self) -> bool {
        self.around.port.state().on_air
    }

    /// What may happen next.
    fn moves(&mut self) -> Vec<Move> {
        let mut moves = Vec::new();
        if self.left.offers > 0 {
            moves.extend((0..TIDS.len()).map(Move::Offer));
        }
        if self.on_air() {
            moves.push(Move::Ended(Answer::Ack));
            if self.left.losses > 0 {
                moves.push(Move::Ended(Answer::NoAck));
                let aggregate = self
                    .around
                    .port
                    .published()
                    .last()
                    .is_some_and(|went| went.single == Went::Aggregate);
                if aggregate {
                    moves.push(Move::Ended(Answer::First));
                    moves.push(Move::Ended(Answer::AllButFirst));
                }
            }
        }
        if self.left.requests > 0 {
            moves.extend(self.requests().into_iter().map(Move::Asked));
        }
        // A command's end and a stop's answer wait for the air to be quiet,
        // as the model's "gate_closed" and "command_done": a turn before it
        // changes nothing.
        if self.owed == Rest::Round || (self.awaits_a_command() && !self.on_air()) {
            moves.push(Move::Turn);
        }
        if self.stop == Stop::Going && !self.on_air() {
            moves.push(Move::StopAgain);
        }
        moves
    }

    fn requests(&mut self) -> Vec<Ask> {
        let mut asks = Vec::new();
        let flowing = self.flowing();
        if flowing && !self.linked() {
            asks.push(Ask::Link);
        }
        let linked = self.linked();
        let held = self.held();
        let naming = self.naming();
        if linked && !held {
            asks.extend([Ask::Hold, Ask::Unlink]);
            for tid in 0..TIDS.len() {
                // An agreement begins where the view offers it: on the TID
                // whose named number was told.
                if self.agreed[tid] {
                    asks.push(Ask::EndAgreement(tid));
                } else if naming == Some(Naming::Named(tid)) {
                    asks.push(Ask::Agree(tid));
                }
                if naming == Some(Naming::Unnamed) {
                    asks.push(Ask::Sequence(tid));
                }
            }
        }
        if linked && held {
            asks.extend([Ask::Release, Ask::Unlink]);
        }
        if linked && self.key_idle() {
            asks.push(Ask::ChangeKey);
        }
        if matches!(naming, Some(Naming::Draining(_) | Naming::Named(_))) {
            asks.push(Ask::Named);
        }
        if flowing && self.gate_open() {
            asks.push(Ask::CloseGate);
        }
        if flowing && self.gate.is_some() {
            asks.push(Ask::OpenGate);
        }
        if !flowing && self.stopped.is_some() {
            asks.push(Ask::Flows);
        }
        if flowing {
            asks.push(Ask::StopData);
        }
        asks
    }

    /// Apply `step`; its problems are kept.
    fn apply(&mut self, step: Step) {
        if let Some(prefix) = step.prefix {
            self.around.port.send_at_most(prefix);
        }
        let before = self.before();
        let rest = match step.action {
            Move::Offer(tid) => {
                self.left.offers -= 1;
                self.offered += 1;
                self.tids.push(tid);
                self.born.push(self.generation);
                self.aired.push(0);
                self.around.net.offer(self.offered, TIDS[tid], DESTINATION);
                Some(self.engine.offered(&mut self.around.turn()))
            }
            Move::Ended(answer) => {
                if answer != Answer::Ack {
                    self.left.losses -= 1;
                }
                let port = &mut self.around.port;
                match answer {
                    Answer::Ack => port.set_air(BEST_EFFORT, Air::Ack),
                    Answer::NoAck => port.set_air(BEST_EFFORT, Air::NoAck),
                    Answer::First => {
                        port.set_air(BEST_EFFORT, Air::Ack);
                        port.acknowledge(FrameSet::prefix(1));
                    }
                    Answer::AllButFirst => {
                        port.set_air(BEST_EFFORT, Air::Ack);
                        port.acknowledge(FrameSet::from_bits(u64::MAX << 1));
                    }
                }
                Some(
                    self.engine
                        .transmitted(TxFacts::DUE, Signal::Quiet, &mut self.around.turn()),
                )
            }
            Move::Asked(ask) => {
                self.left.requests -= 1;
                self.ask(ask)
            }
            Move::Turn => Some(self.engine.round(&mut self.around.turn())),
            Move::StopAgain => {
                self.stop_data();
                None
            }
        };
        self.owed = rest.unwrap_or(Rest::Wake(None));
        if let Some(gate) = self.around.told.gate.take() {
            self.gate = Some(gate);
            self.closing = Awaited::No;
        }
        for end in &self.around.told.keys[self.keys_told..] {
            self.keying = Awaited::No;
            self.key = match end {
                KeyEnd::Installed => KeyTold::Installed,
                KeyEnd::Expired | KeyEnd::Removed => KeyTold::Empty,
            };
        }
        self.keys_told = self.around.told.keys.len();
        for (tid, number) in self.around.told.named.drain(..) {
            let tid = tid_index(tid);
            if self.hold != Some(tid) {
                self.problems
                    .push("a number was told for a TID nobody named");
            }
            self.named.push((tid, number.get()));
            if let Some(carried) = self.carried.get_mut(tid) {
                *carried = Some(number.get());
            }
        }
        self.stop_awaits();
        self.check_step(&before, step.action);
        self.check(rest);
    }

    /// What a step is checked against: the world before it.
    fn before(&self) -> Before {
        let arenas = self.arena_ids();
        let quiet = !self.on_air();
        let kept = arenas
            .iter()
            .copied()
            .filter(|id| quiet && self.aired_of(*id) > 0)
            .collect();
        let single = match self.around.port.published().last() {
            Some(went) if !quiet && went.single == Went::Single => {
                self.on_air_ids().first().copied()
            }
            _ => None,
        };
        Before {
            published: self.around.port.published().len(),
            keying: self.keying,
            arenas,
            kept,
            single,
            hold: self.hold,
        }
    }

    /// The MSDUs the arenas hold.
    fn arena_ids(&self) -> Vec<u32> {
        [ArenaSide::First, ArenaSide::Second]
            .into_iter()
            .flat_map(|side| self.around.port.arena_memories(side))
            .collect()
    }

    fn aired_of(&self, id: u32) -> u8 {
        self.aired.get(id as usize - 1).copied().unwrap_or(0)
    }

    /// The MSDUs of the arena on the air, in its order: a publication's.
    fn on_air_ids(&self) -> Vec<u32> {
        self.around
            .port
            .on_air_side()
            .map(|side| self.around.port.arena_memories(side))
            .unwrap_or_default()
    }

    /// The model's checks of a publication and of the arenas, from what the
    /// world sees: the publications, the arenas' MSDUs and their fronts.
    fn check_step(&mut self, before: &Before, action: Move) {
        let published: Vec<Vec<Vec<u8>>> = self.around.port.published()[before.published..]
            .iter()
            .map(|went| went.frames.clone())
            .collect();
        let after = self.arena_ids();
        // A named TID's intake waits until its agreement begins or its name
        // is released.
        let unholds = matches!(action, Move::Asked(Ask::Agree(_) | Ask::Named));
        if let Some(tid) = before.hold
            && !unholds
            && after.iter().any(|id| {
                !before.arenas.contains(id) && self.tids.get(*id as usize - 1) == Some(&tid)
            })
        {
            self.problems
                .push("an MSDU of a named TID was written before its hold ended");
        }
        for (index, frames) in published.iter().enumerate() {
            if before.keying == Awaited::Yes && self.keying == Awaited::Yes {
                self.problems
                    .push("an arena went while a key change awaited");
            }
            // One arena on the air at a time: the step's publication is the
            // arena on the air after it, its frames the prefix that went.
            let ids: Vec<u32> = self.on_air_ids().into_iter().take(frames.len()).collect();
            if index == 0
                && !before.kept.is_empty()
                && !ids.iter().any(|id| before.kept.contains(id))
            {
                self.problems
                    .push("the standby went before an arena kept for its retry");
            }
            for (front, id) in frames.iter().zip(&ids) {
                if self.born.get(*id as usize - 1) != Some(&self.generation) {
                    self.problems.push("an ended link's frame went");
                }
                let up = front[24] & 0x07;
                let Some(tid) = TIDS.iter().position(|tid| tid.value() == up) else {
                    continue;
                };
                if self.agreed[tid] {
                    let sequence = |front: &[u8]| {
                        SequenceNumber::from_sequence_control(
                            oer_protocols_ieee80211_wire::sequence::SequenceControl::from_bytes([
                                front[22], front[23],
                            ]),
                        )
                        .get()
                    };
                    let oldest = after
                        .iter()
                        .filter(|held| self.born.get(**held as usize - 1) == Some(&self.generation))
                        .filter_map(|held| self.around.port.front(*held))
                        .filter(|held| held[24] & 0x07 == up)
                        .map(sequence)
                        .min();
                    if oldest.is_some_and(|oldest| sequence(front) - oldest >= WINDOW) {
                        self.problems.push("a frame went beyond its window");
                    }
                }
            }
            for id in ids {
                if let Some(count) = self.aired.get_mut(id as usize - 1) {
                    *count += 1;
                }
            }
        }
        if before.keying == Awaited::Yes
            && self.keying == Awaited::Yes
            && after.iter().any(|id| !before.arenas.contains(id))
        {
            self.problems
                .push("an MSDU was written while a key change awaited");
        }
        if let (Move::Ended(Answer::NoAck), Some(single)) = (action, before.single)
            && after.contains(&single)
        {
            self.problems
                .push("a frame that went as one MPDU and missed was kept");
        }
        if !self.on_air() && after.iter().any(|id| self.aired_of(*id) >= ATTEMPTS) {
            self.problems.push("a frame is kept with no attempt left");
        }
        self.check_window(&after);
        self.check_blocked(&after);
    }

    /// No MSDU numbered beyond its TID's window is in an arena: the window
    /// begins at the oldest number the arenas hold of the TID, and one
    /// beyond it waits in its queue.
    fn check_window(&mut self, arenas: &[u32]) {
        for (tid, up) in TIDS.iter().enumerate() {
            if !self.agreed[tid] {
                continue;
            }
            let numbers: Vec<u16> = arenas
                .iter()
                .filter(|id| self.born.get(**id as usize - 1) == Some(&self.generation))
                .filter_map(|id| self.around.port.front(*id))
                .filter(|front| front[24] & 0x07 == up.value())
                .map(|front| {
                    SequenceNumber::from_sequence_control(
                        oer_protocols_ieee80211_wire::sequence::SequenceControl::from_bytes([
                            front[22], front[23],
                        ]),
                    )
                    .get()
                })
                .collect();
            if let (Some(oldest), Some(newest)) = (numbers.iter().min(), numbers.iter().max())
                && newest - oldest >= WINDOW
            {
                self.problems
                    .push("an MSDU numbered beyond its TID's window is in an arena");
            }
        }
    }

    /// A TID held by its window moves when its frames in the arenas
    /// settle: with none there, the air quiet and nothing awaited, its
    /// queued MSDUs go.
    fn check_blocked(&mut self, arenas: &[u32]) {
        if self.owed == Rest::Round
            || self.on_air()
            || self.awaits_a_command()
            || self.hold.is_some()
            || !self.linked()
            || self.held()
            || !self.gate_open()
        {
            return;
        }
        let home = &self.around.net.home;
        let waiting: Vec<u32> = self
            .around
            .net
            .offered
            .iter()
            .map(|memory| memory.id)
            .collect();
        for tid in 0..TIDS.len() {
            let mut ids = (1..=self.offered).filter(|id| self.tids[*id as usize - 1] == tid);
            let queued = ids
                .clone()
                .any(|id| !home.contains(&id) && !arenas.contains(&id) && !waiting.contains(&id));
            let in_arenas = ids.any(|id| arenas.contains(&id));
            if self.agreed[tid] && queued && !in_arenas {
                self.problems
                    .push("a TID is blocked by its window and nothing can move it");
            }
        }
    }

    fn ask(&mut self, ask: Ask) -> Option<Rest> {
        if ask == Ask::StopData {
            self.agreed = [false; 2];
            self.hold = None;
            self.stop_data();
            return None;
        }
        let mut turn = self.around.turn();
        let refused = "a request the engine's state admits was refused";
        let answer = match ask {
            Ask::Flows => {
                let DataPlace::Stopped(data) = self.engine.data() else {
                    self.problems.push(refused);
                    return None;
                };
                let stopped = self.stopped.take()?;
                self.stop = Stop::NotAsked;
                let _ = data.flows(stopped);
                return None;
            }
            Ask::CloseGate | Ask::OpenGate => {
                let DataPlace::Flowing(mut flowing) = self.engine.data() else {
                    self.problems.push(refused);
                    return None;
                };
                match (ask, flowing.gate()) {
                    (Ask::CloseGate, GatePlace::Open(open)) => {
                        self.closing = Awaited::Yes;
                        Some(open.close(&mut turn))
                    }
                    (Ask::OpenGate, GatePlace::Shut(shut)) => {
                        let gate = self.gate.take()?;
                        Some(shut.open(gate, &mut turn))
                    }
                    _ => None,
                }
            }
            Ask::Link => {
                let DataPlace::Flowing(mut flowing) = self.engine.data() else {
                    self.problems.push(refused);
                    return None;
                };
                let PeerPlace::Free(free) = flowing.peer(ACCESS_POINT) else {
                    self.problems.push(refused);
                    return None;
                };
                let owed = free.link(station_link(ATTEMPTS, HOUR), &mut turn);
                self.next = [None; 2];
                self.agreed = [false; 2];
                self.hold = None;
                self.named.clear();
                self.carried = [None; 2];
                self.link_from = (
                    self.around.port.published().len(),
                    self.around.net.home.len(),
                );
                self.generation += 1;
                Some(owed)
            }
            ask => {
                let DataPlace::Flowing(mut flowing) = self.engine.data() else {
                    self.problems.push(refused);
                    return None;
                };
                let PeerPlace::Linked(mut linked) = flowing.peer(ACCESS_POINT) else {
                    self.problems.push(refused);
                    return None;
                };
                match ask {
                    Ask::Hold => Some(linked.hold(&mut turn)),
                    Ask::Release => Some(linked.release(&mut turn)),
                    Ask::Unlink => {
                        self.agreed = [false; 2];
                        self.hold = None;
                        Some(linked.unlink(&mut turn))
                    }
                    Ask::Agree(tid) => match linked.naming() {
                        NamePlace::Named(named) if named.tid() == TIDS[tid] => {
                            self.agreed[tid] = true;
                            self.hold = None;
                            Some(named.tx_agreement(WINDOW, &mut turn))
                        }
                        _ => {
                            self.problems.push(refused);
                            None
                        }
                    },
                    Ask::EndAgreement(tid) => {
                        self.agreed[tid] = false;
                        Some(linked.tx_agreement_ended(TIDS[tid], &mut turn))
                    }
                    Ask::Sequence(tid) => match linked.naming() {
                        NamePlace::Unnamed(unnamed) => {
                            self.hold = Some(tid);
                            match unnamed.name(TIDS[tid], &mut turn) {
                                Ok(rest) => Some(rest),
                                Err(NoSpace) => {
                                    self.problems.push(refused);
                                    None
                                }
                            }
                        }
                        _ => {
                            self.problems.push(refused);
                            None
                        }
                    },
                    Ask::Named => {
                        self.hold = None;
                        match linked.naming() {
                            NamePlace::Draining(draining) => Some(draining.release(&mut turn)),
                            NamePlace::Named(named) => Some(named.release(&mut turn)),
                            NamePlace::Unnamed(_) => {
                                self.problems.push(refused);
                                None
                            }
                        }
                    }
                    Ask::ChangeKey => match linked.key() {
                        KeyPlace::Idle(idle) => {
                            self.keying = Awaited::Yes;
                            Some(idle.change(key_change(1), &mut turn))
                        }
                        _ => {
                            self.problems.push(refused);
                            None
                        }
                    },
                    _ => None,
                }
            }
        };
        if ask == Ask::Link {
            self.engine.aggregate_rate(&AggregateRate {
                rate: ht(),
                max_subframes: CAPACITY,
                max_length: 65_535,
            });
        }
        answer
    }

    /// What a stop the world asked awaits, from what it knows: a key it
    /// was told installed is removed, a gate it lent is closed.
    fn stop_awaits(&mut self) {
        if self.stop != Stop::Going {
            return;
        }
        if self.key == KeyTold::Installed {
            self.keying = Awaited::Yes;
        }
        if self.gate.is_none() {
            self.closing = Awaited::Yes;
        }
    }

    /// Whether the world awaits a command's end it asked for.
    fn awaits_a_command(&self) -> bool {
        self.keying == Awaited::Yes || self.closing == Awaited::Yes
    }

    fn stop_data(&mut self) {
        let again = self.stop == Stop::Going;
        if self.stop == Stop::NotAsked {
            self.stop = Stop::Going;
            self.stop_awaits();
        }
        let quiet = !self.on_air();
        let told = (self.around.told.gate.is_some(), self.around.told.keys.len());
        let mut turn = self.around.turn();
        let answer = match self.engine.data() {
            DataPlace::Flowing(flowing) => flowing.stop(&mut turn),
            DataPlace::Stopping(stopping) => stopping.poll(&mut turn),
            DataPlace::Stopped(_) => {
                self.problems.push("a stop of stopped data was asked");
                return;
            }
        };
        match answer {
            DataStop::Stopped(stopped) => {
                self.stopped = Some(stopped);
                self.stop = Stop::Stopped;
            }
            DataStop::Going => {
                self.stop = Stop::Going;
                // A stop goes on only while something ends: the air, or a
                // command the world awaits.
                if !self.on_air() && !self.awaits_a_command() {
                    self.problems
                        .push("a stop goes on with nothing on the air and no command");
                }
                // The port ends a command at its first poll with the air
                // quiet: a stop asked again then ends one of those the
                // world awaits.
                let told_now = (self.around.told.gate.is_some(), self.around.told.keys.len());
                if again && quiet && !self.on_air() && told_now == told {
                    self.problems
                        .push("a stop asked again with the air quiet ends nothing");
                }
            }
        }
    }

    /// The MSDUs in each place.
    fn places(&self) -> (Vec<u32>, Vec<u32>, usize) {
        let arenas = [ArenaSide::First, ArenaSide::Second]
            .into_iter()
            .flat_map(|side| self.around.port.arena_memories(side))
            .collect();
        let waiting = self
            .around
            .net
            .offered
            .iter()
            .map(|memory| memory.id)
            .collect();
        (arenas, waiting, self.engine.queued())
    }

    fn check(&mut self, rest: Option<Rest>) {
        let (arenas, waiting, queued) = self.places();
        let home = &self.around.net.home;
        // Memory comes home once: every offered MSDU is in exactly one place.
        let mut sorted = home.clone();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted.len() != home.len() {
            self.problems.push("an MSDU came home twice");
        }
        if arenas.iter().any(|id| home.contains(id)) {
            self.problems.push("an MSDU is home and in an arena");
        }
        if home.len() + arenas.len() + waiting.len() + queued != self.offered as usize {
            self.problems.push("an MSDU is in two places or in none");
        }
        // A stop from every state.
        if self.stop == Stop::Stopped && !self.flowing() {
            if queued != 0 || !arenas.is_empty() {
                self.problems.push("the data stopped with memory out");
            }
            if self.gate_not_closed() {
                self.problems
                    .push("the data stopped with the gate not closed");
            }
            if self.linked() {
                self.problems.push("the data stopped with a link left");
            }
        }
        if !self.linked() && queued != 0 {
            self.problems.push("a link ended with its MSDUs waiting");
        }
        // Order a TID: each link's first transmissions of a TID carry
        // consecutive numbers, a number a sequence request took aside.
        self.check_order();
        // Nothing waits for a call nobody makes.
        if rest == Some(Rest::Wake(None)) && !self.awaits_a_command() && self.flowing() {
            let before = (
                self.around.net.home.len(),
                self.around.port.published().len(),
                self.places(),
            );
            let again = self.engine.round(&mut self.around.turn());
            let after = (
                self.around.net.home.len(),
                self.around.port.published().len(),
                self.places(),
            );
            if before != after || again == Rest::Round {
                self.problems.push("work waits for a call nobody makes");
            }
        }
    }

    fn check_order(&mut self) {
        let published = self.around.port.published();
        for went in published.iter().skip(self.checked) {
            for front in &went.frames {
                let retried = front[1] & 0x08 != 0;
                if retried {
                    continue;
                }
                let control =
                    oer_protocols_ieee80211_wire::sequence::SequenceControl::from_bytes([
                        front[22], front[23],
                    ]);
                let sequence = SequenceNumber::from_sequence_control(control).get();
                let up = front[24] & 0x07;
                let Some(tid) = TIDS.iter().position(|tid| tid.value() == up) else {
                    self.problems.push("a frame of a TID nobody offered went");
                    continue;
                };
                let mut expected = self.next[tid].unwrap_or(0);
                while self.withdrawn_below_a_named_number(tid, expected) {
                    expected += 1;
                }
                if sequence != expected {
                    self.problems
                        .push("a TID's first transmissions are out of order or have a gap");
                }
                if let Some(told) = self.carried[tid].take()
                    && sequence != told
                {
                    self.problems
                        .push("the first transmission after a named number does not carry it");
                }
                self.next[tid] = Some(sequence + 1);
            }
        }
        self.checked = published.len();
    }

    /// Whether `sequence` of `tid` is a number this link drew for an MSDU
    /// that went home before any transmission, below a number told for a
    /// named TID: it named nothing to anyone, and the peer counts from the named
    /// one.
    fn withdrawn_below_a_named_number(&self, tid: usize, sequence: u16) -> bool {
        let named_above = self
            .named
            .iter()
            .any(|&(named, number)| named == tid && sequence < number);
        if !named_above {
            return false;
        }
        let (published_from, home_from) = self.link_from;
        let numbered = |front: &[u8]| {
            let control = oer_protocols_ieee80211_wire::sequence::SequenceControl::from_bytes([
                front[22], front[23],
            ]);
            (
                front[24] & 0x07,
                SequenceNumber::from_sequence_control(control).get(),
            )
        };
        let wanted = (TIDS[tid].value(), sequence);
        let transmitted = self.around.port.published()[published_from..]
            .iter()
            .flat_map(|went| &went.frames)
            .any(|front| numbered(front) == wanted);
        let withdrawn = self.around.net.home[home_from..]
            .iter()
            .filter_map(|id| self.around.port.front(*id))
            .any(|front| numbered(front) == wanted);
        withdrawn && !transmitted
    }

    /// The whole state: the world's, the engine's private fields and the
    /// model port's. Equal keys behave alike.
    fn key(&mut self) -> String {
        let mut home = self.around.net.home.clone();
        home.sort_unstable();
        let waiting: Vec<u32> = self
            .around
            .net
            .offered
            .iter()
            .map(|memory| memory.id)
            .collect();
        std::format!(
            "{:?}|{}|{:?}|{:?}|{:?}|{:?}|{}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}",
            (self.left.offers, self.left.losses, self.left.requests),
            self.offered,
            self.owed,
            self.stop,
            self.agreed,
            self.hold,
            self.gate.is_some(),
            self.stopped.is_some(),
            self.tids,
            waiting,
            home,
            self.next,
            (&self.named, self.carried),
            self.link_from,
            (self.closing, self.keying, self.key),
            (self.generation, &self.born, &self.aired),
            self.engine.key(),
            self.around.port.state_key(),
        )
    }
}

/// What a replayed path found at its last step.
struct Visit {
    problems: Vec<&'static str>,
    moves: Vec<Move>,
    key: String,
    /// The frames the last step's first publication sent, when it chose its
    /// prefix freely.
    sent: Option<u8>,
}

fn replay(path: &[Step], bounds: Bounds) -> Visit {
    running::<StationModel<STATION_SINGLES>, _>(|run| {
        let mut world = World::new(run, bounds);
        let mut sent = None;
        for (index, step) in path.iter().enumerate() {
            let last = index + 1 == path.len();
            if last {
                world.problems.clear();
            }
            let before = world.around.port.published().len();
            world.apply(*step);
            if last && step.prefix.is_none() {
                sent = world
                    .around
                    .port
                    .published()
                    .get(before)
                    .map(|went| went.frames.len() as u8);
            }
        }
        Visit {
            problems: core::mem::take(&mut world.problems),
            moves: world.moves(),
            key: world.key(),
            sent,
        }
    })
}

/// What a search found: its size and each problem with a path to it.
pub(super) struct Found {
    /// Distinct states.
    pub nodes: usize,
    /// Steps taken: each a section from a state, checked.
    pub ways: usize,
    pub depth: usize,
    pub problems: Vec<(&'static str, String)>,
}

pub(super) fn search(bounds: Bounds, ceiling: usize) -> Found {
    let mut found = Found {
        nodes: 0,
        ways: 0,
        depth: 0,
        problems: Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    let mut stack: Vec<Vec<Step>> = std::vec![Vec::new()];
    while let Some(path) = stack.pop() {
        let visit = replay(&path, bounds);
        found.ways += 1;
        found.depth = found.depth.max(path.len());
        for problem in visit.problems {
            if !found.problems.iter().any(|(seen, _)| *seen == problem) {
                found.problems.push((problem, std::format!("{path:?}")));
            }
        }
        if let (Some(sent), Some(last)) = (visit.sent, path.last()) {
            for prefix in 1..sent {
                let mut sibling = path.clone();
                if let Some(step) = sibling.last_mut() {
                    *step = Step {
                        action: last.action,
                        prefix: Some(prefix),
                    };
                }
                stack.push(sibling);
            }
        }
        // A state reached before is not explored again; the step into it
        // was checked above.
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
            if !visit.moves.is_empty()
                && !found
                    .problems
                    .iter()
                    .any(|(seen, _)| *seen == "a path ran past its bound")
            {
                found
                    .problems
                    .push(("a path ran past its bound", std::format!("{path:?}")));
            }
            continue;
        }
        for action in visit.moves {
            let mut child = path.clone();
            child.push(Step {
                action,
                prefix: None,
            });
            stack.push(child);
        }
    }
    found
}

/// The most nodes a search at the ordinary bounds visits (about 12 000
/// today) and one at named bounds: ceilings against an explosion, several
/// times what they take, never a pinned figure.
const ORDINARY_CEILING: usize = 100_000;
const NAMED_CEILING: usize = 3_000_000;

/// Run the search at `bounds`, print its size, time and problems.
fn run(bounds: Bounds, ceiling: usize) {
    let start = Clock::now();
    let found = search(bounds, ceiling);
    std::println!(
        "search {bounds:?}: {} states, {} ways, depth {}, {:?}",
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
fn every_order_of_the_station_engines_sections_keeps_its_properties() {
    run(
        Bounds {
            offers: 3,
            losses: 1,
            requests: 4,
        },
        ORDINARY_CEILING,
    );
}

/// The search at the bounds the model's mutants run at (two links and an
/// agreement on the second need six requests). Named, not run by default:
/// its size and time are in the pull request.
#[test]
#[ignore = "the mutants' bounds: run by name"]
fn every_order_at_the_mutants_bounds() {
    run(
        Bounds {
            offers: 3,
            losses: 1,
            requests: 6,
        },
        NAMED_CEILING,
    );
}

/// The bounds where the window binds: a fourth MSDU behind a full window,
/// as the model's mutant of the intake's window check is run at.
#[test]
#[ignore = "the window's bounds: run by name"]
fn every_order_at_the_windows_bounds() {
    run(
        Bounds {
            offers: 4,
            losses: 1,
            requests: 4,
        },
        NAMED_CEILING,
    );
}

/// The bounds where a frame spends every attempt: two losses, as the
/// model's mutant of the attempts is run at.
#[test]
#[ignore = "the attempts' bounds: run by name"]
fn every_order_at_the_attempts_bounds() {
    run(
        Bounds {
            offers: 3,
            losses: 2,
            requests: 6,
        },
        NAMED_CEILING,
    );
}
