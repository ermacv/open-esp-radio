#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

//! A model of hardware behind the Bluetooth LE port's contract
//! (`design/ble-port-contract-2026-11-19.md` of the notes repository), the
//! subject of the conformance suite.
//!
//! [`ModelPort`] stands where a chip's port stands: it implements
//! [`LePort`] and its round's parts over a modelled controller:
//!
//! - a scheduler with one list and a clock of its own that the test moves
//!   ([`ModelPort::advance`]). An event published later than its start
//!   less the model's guard ([`MODEL_GUARD`]) ends `NotExecuted(Late)`; an
//!   event the test tells it not to run ([`ModelPort::displace`]) ends
//!   `NotExecuted(Displaced)` at its start;
//! - an air with a scripted peer for each role ([`ModelPort::script`]): a
//!   scanner that asks or connects, an advertiser to be scanned, a central
//!   that sends, acknowledges, withholds its acknowledgement and loses
//!   packets, and a tester's packets;
//! - a connection's transmit chain and receive nodes, by the geometry;
//! - a withdraw that ends what is out as the events model has it: an event
//!   not begun `NotExecuted(Withdrawn)`, one begun runs to its end, every
//!   PDU of the chain `Withdrawn`;
//! - each anomaly a backend may indicate, raisable at any point
//!   ([`ModelPort::raise`]); after it a round is empty until the stop;
//! - a stop by the contract's one path: the stop's request, then the
//!   blocks' reset, which ends every event out (`NotExecuted(Withdrawn)` if
//!   the scheduler had not begun it, `Indeterminate` if it had) and every
//!   PDU in a chain (`Withdrawn`) through the rounds the stop leaves. There
//!   is no wait for the scheduler. After the reset the modelled hardware is
//!   as the install found it, so a start is always cold;
//! - a record of what it was asked to program and what it answered in each
//!   run ([`ModelPort::runs`]), for "a second run is as the first";
//! - counts of its calls ([`ModelPort::counts`]).
//!
//! It holds state about its modelled hardware only (rule 7 of the
//! contract): its phase, the scheduler's list, the roles' images in the
//! memory the CPU holds, the chains, the nodes, what ended and was not read
//! yet, its latched facts, its record and its counts.
//!
//! # A round abandoned
//!
//! A [`Round`](oer_protocols_bluetooth_le_contracts_port::Round) dropped
//! with no close is a defect of the code that dropped it, not of the
//! hardware; the model does not repair it. What the abandoned round had not
//! given stays for the next round: the ends and the transmit ends it did not
//! give, in their order. A reception it gave and that was not taken stays
//! out of the hardware until the next round's close gives its node back. An
//! event whose `Ended` was dropped took its place with it: the model holds
//! no copy of a right.
//!
//! # Receptions not read
//!
//! The receptions of an `Ended` not read before the next `ended`, and a
//! `Pdu` neither taken nor read, go back to the hardware at the round's
//! close.
//!
//! # Assumptions about the hardware
//!
//! Where the stand has not answered, the model assumes, each with the
//! contract's open point:
//!
//! - **B4**: an event listed while the scheduler runs another runs at its
//!   time; the model's list is ordered by start and serves every listed
//!   event at its start.
//! - **B5**: a transmit's end is the peer's acknowledgement: a PDU is
//!   `Transmitted` in the event whose central acknowledged it, and not
//!   before.
//! - **B6**: with no receive node free, the peer's PDU is not taken and not
//!   acknowledged: the central sends it again in its next event.
//! - **B10**: an event's length is bounded by its window: an event runs
//!   from its window's start to its window's end, whatever the interval.
//! - **B11**: an event the hardware does not run is `Displaced` only when
//!   the test says so; the model displaces nothing by itself.

extern crate std;

use core::task::Context;
use std::collections::VecDeque;
use std::vec::Vec;

use oer_protocols_bluetooth_le_contracts_port::{
    AcceptEntries, AcceptList, AdvEvent, AdvPlace, Advertiser, AdvertisingData, AdvertisingEvent,
    AdvertisingSet, ConnEvent, ConnPlace, Connection, ConnectionEvent, ConnectionSetup, Cut,
    EventEnd, EventPlace, Executed, Facts, LeCapabilities, LeConnectionCapabilities, LeInstant,
    LeLimits, LePhy, LePhys, LePort, LeRadioCapabilities, LeRounds, LegacyAdvertisingTypes,
    LinkAcknowledgement, MoreData, NotExecuted, PduHandle, PduLen, PortKey, Published,
    RadioDuration, RadioTiming, Received, Rest, Retired, RoleRef, RoleRights, RoundSeal, ScanEvent,
    ScanPlace, ScanWindow, Scanner, ScannerSetup, Signal, StartFailure, StartPoll, Starting,
    StopFrom, StopPoll, StopRight, Stopped, Stopping, Supported, Test, TestEvent, TestPlace,
    TestWindow, TxEnd, TxPlace, dtm::DtmTest,
};
use oer_protocols_bluetooth_le_wire::DataPduKind;

/// The longest data channel payload the model carries.
pub const MODEL_PAYLOAD: u8 = 251;

/// The model's guard: an event whose reservation starts sooner than this
/// after the model's clock is late.
pub const MODEL_GUARD: RadioDuration<oer_protocols_bluetooth_le_contracts_port::LeRadio> =
    RadioDuration::from_micros(50);

/// What the model serves: every role, LE 1M and 2M, a list and room for the
/// geometries the tests use.
pub const MODEL_CAPABILITIES: LeRadioCapabilities = LeRadioCapabilities {
    legacy_advertising: LegacyAdvertisingTypes {
        nonconnectable: true,
        responsive: true,
    },
    passive_scanning: true,
    active_scanning: true,
    filter_accept_list: true,
    peripheral_connection: Some(LeConnectionCapabilities {
        max_data_payload: MODEL_PAYLOAD,
        link_acknowledgement: LinkAcknowledgement::Hardware,
        max_first_receive_wait: RadioDuration::from_micros(65_473),
        max_receive_wait: RadioDuration::from_micros(131_070),
    }),
    direct_test_mode: true,
    phys: LePhys::LE_1M.with(LePhy::Le2M),
    test_phys: LePhys::LE_1M.with(LePhy::Le2M),
    min_tx_power_dbm: -20,
    max_ahead: RadioDuration::from_micros(10_000_000),
    timing: RadioTiming {
        preparation_lead: RadioDuration::from_micros(100),
        admission_guard: MODEL_GUARD,
        ..RadioTiming::MINIMAL
    },
    limits: LeLimits {
        advertising_sets: 4,
        connections: 4,
        connection_events: 4,
        transmit_places: 8,
        receive_nodes: 8,
        accept_list: 8,
    },
};

/// The model's capabilities marker.
pub enum ModelCaps {}

impl LeCapabilities for ModelCaps {
    const CAPS: LeRadioCapabilities = MODEL_CAPABILITIES;
}

/// A role of the model's list.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Role {
    /// An advertising set.
    Advertising,
    /// The scanner.
    Scan,
    /// A connection.
    Connection,
    /// The test session.
    Test,
}

/// One event place of one instance of a role.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EventId {
    /// Its role.
    pub role: Role,
    /// Its instance.
    pub instance: u8,
    /// Its slot: a connection's event place; 0 for the other roles.
    pub slot: u8,
}

impl EventId {
    fn of(place: &EventPlace<'_>) -> Self {
        let (role, instance, slot) = match place {
            EventPlace::Advertising(place) => (Role::Advertising, place.index(), 0),
            EventPlace::Scan(place) => (Role::Scan, place.index(), 0),
            EventPlace::Connection(place) => (Role::Connection, place.index(), place.slot()),
            EventPlace::Test(place) => (Role::Test, place.index(), 0),
        };
        Self {
            role,
            instance,
            slot,
        }
    }
}

/// An anomaly a backend may indicate: the S31's closed set, by kind
/// (`design/ble-port-contract-2026-11-19.md`, section 4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Anomaly {
    /// An exception lane of the primary handler.
    ExceptionLane,
    /// A completed receive node still holding the armed sentinel.
    Sentinel,
    /// The scheduler ended with an unexecuted item in a published list.
    UnexecutedLeft,
    /// An execution lock, a modify or a skip answered what it must not.
    LockAnswer,
    /// A hardware transaction did not end in its bound.
    Bound,
}

/// What the next start does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartScript {
    /// It confirms.
    Confirm,
    /// It fails by this cause.
    Fail(StartFailure),
}

/// Whether a scripted peer's packet is acknowledged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ack {
    /// The peer acknowledges the head of the chain.
    Acknowledge,
    /// The peer withholds its acknowledgement.
    Withhold,
}

/// Whether a scripted peer's packets reach the model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Heard {
    /// They do.
    Heard,
    /// They are lost: the event runs and hears nothing.
    Lost,
}

/// What a scripted peer does in one event of its role.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Turn {
    /// A scanner or an initiator answers an advertising event with this
    /// PDU (`SCAN_REQ` or `CONNECT_IND`), header first.
    Request(Vec<u8>),
    /// Advertisers heard in a scan window, each PDU header first.
    Advertisements(Vec<Vec<u8>>),
    /// The central of a connection event.
    Central {
        /// Whether it is heard.
        heard: Heard,
        /// Whether it acknowledges the chain's head.
        ack: Ack,
        /// The PDU it sends, header first; empty for nothing.
        sends: Vec<u8>,
    },
    /// The packets a tester sends in a receiver test's window with a good
    /// CRC.
    Packets(u8),
}

/// One entry of a run's record: what the model was asked to program and
/// what it answered, with instants as offsets from the run's start.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Programmed {
    /// The start.
    Start,
    /// The start's end.
    Started(Option<StartFailure>),
    /// A role's image written.
    Opened(Role, u8),
    /// A role's image taken down.
    Closed(Role, u8),
    /// An advertising set's data written again.
    Rewritten(u8),
    /// The accept list written, with its length.
    AcceptList(usize),
    /// An event listed, at its window's start from the run's start.
    Published(
        EventId,
        RadioDuration<oer_protocols_bluetooth_le_contracts_port::LeRadio>,
    ),
    /// A role's events and chain withdrawn.
    Withdrawn(Role, u8),
    /// A PDU appended to a connection's chain.
    Submitted {
        /// The connection.
        instance: u8,
        /// The transmit place.
        slot: u8,
        /// Its LLID.
        llid: DataPduKind,
        /// Its payload.
        payload: Vec<u8>,
    },
    /// An event's end given in a round, with its anchor from the run's
    /// start.
    Ended(EventId, Ending),
    /// A PDU's end given in a round.
    Transmitted(u8, u8, TxEnd),
    /// A PDU taken in a round, by its node.
    Taken(u16),
    /// A PDU released, by its node.
    Released(u16),
    /// The stop's request.
    Stop,
    /// The blocks' reset.
    Reset,
}

/// An event's end as a run's record holds it: its anchor from the run's
/// start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ending {
    /// It ran.
    Executed {
        /// The captured anchor from the run's start.
        anchor: Option<RadioDuration<oer_protocols_bluetooth_le_contracts_port::LeRadio>>,
        /// The packets exchanged.
        exchanged: u8,
        /// Whether it was cut.
        cut: Cut,
        /// The peer's last More Data bit.
        peer: Option<MoreData>,
    },
    /// It did not run.
    NotExecuted(NotExecuted),
    /// It may have been running when the scheduler stopped.
    Indeterminate,
}

/// The model's counts of its calls.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counts {
    /// Every call of the port, the round's parts included.
    pub calls: u32,
    /// The rounds opened.
    pub rounds: u32,
    /// The publications.
    pub publications: u32,
    /// The PDUs written.
    pub writes: u32,
    /// The PDUs submitted.
    pub submissions: u32,
}

/// The phase of the modelled controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// Stopped: as the install or the last reset left it.
    Stopped,
    /// A start in progress, since the run's start.
    Starting(LeInstant),
    /// Running, since the run's start.
    Running(LeInstant),
    /// The stop's request taken, since the run's start.
    Stopping(LeInstant),
    /// Retired.
    Retired,
    /// Released.
    Released,
}

/// Where a listed event is in the scheduler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Run {
    /// Listed, not begun.
    Waiting,
    /// Listed, and the test told the model not to run it.
    Displaced,
    /// Begun.
    Begun,
}

/// One event in the scheduler's list, with its place.
#[derive(Debug)]
struct Listed<'id> {
    place: EventPlace<'id>,
    start: LeInstant,
    end: LeInstant,
    run: Run,
    /// The packets the event sends by itself: an advertising event's
    /// channels, a transmitter test's packets.
    sends: u8,
}

/// One event that ended, held for a round.
#[derive(Debug)]
struct Finished<'id> {
    place: EventPlace<'id>,
    end: EventEnd,
    receptions: VecDeque<u16>,
}

/// A received PDU in its node.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Reception {
    header: [u8; 2],
    payload: Vec<u8>,
    captured_at: Option<LeInstant>,
}

/// One receive node.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Node {
    /// The hardware may fill it.
    Free,
    /// Filled, held for a round with its event's end.
    Filled(Reception),
    /// Given in a round, not taken.
    Given(Reception),
    /// Taken: its handle is held.
    Held(Reception),
    /// Released: it goes back at the round's close.
    Released,
}

/// The fact that the port needs a stop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NeedsStop {
    /// Not raised since the last stop.
    Quiet,
    /// Raised by this anomaly, latched until the stop.
    Raised(Anomaly),
}

/// The port's one alarm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Alarm {
    /// Not armed.
    Disarmed,
    /// Armed at this instant.
    At(LeInstant),
}

/// A role instance's image in the memory the CPU holds.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Image {
    /// Closed.
    Closed,
    /// Open.
    Open,
}

/// The model of hardware behind the contract, of the geometry `ADV`,
/// `SCAN`, `CONN`, `EVENTS`, `TX`, `RX`, `TEST`.
pub struct ModelPort<
    'id,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const RX: usize,
    const TEST: usize,
> {
    key: PortKey<'id, EVENTS, TX>,
    phase: Phase,
    now: LeInstant,
    starts: VecDeque<StartScript>,
    list: Vec<Listed<'id>>,
    finished: VecDeque<Finished<'id>>,
    reading: VecDeque<u16>,
    images: [Vec<Image>; 4],
    chains: Vec<VecDeque<TxPlace<'id>>>,
    memory: Vec<(DataPduKind, Vec<u8>)>,
    transmitted: VecDeque<(TxEnd, TxPlace<'id>)>,
    nodes: Vec<Node>,
    scripts: Vec<((Role, u8), VecDeque<Turn>)>,
    needs_stop: NeedsStop,
    alarm: Alarm,
    record: Vec<Programmed>,
    runs: Vec<Vec<Programmed>>,
    counts: Counts,
}

/// The shared chains' order after the connections' nodes.
#[derive(Clone, Copy)]
enum Chain {
    Connection(u8),
    Advertising,
    Scan,
}

impl<
    'id,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const RX: usize,
    const TEST: usize,
> ModelPort<'id, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>
{
    /// The model, built from the installation's key, its clock at `now`.
    pub fn new(key: PortKey<'id, EVENTS, TX>, now: LeInstant) -> Self {
        let nodes = CONN.saturating_add(2).saturating_mul(RX);
        Self {
            key,
            phase: Phase::Stopped,
            now,
            starts: VecDeque::new(),
            list: Vec::new(),
            finished: VecDeque::new(),
            reading: VecDeque::new(),
            images: [
                std::vec![Image::Closed; ADV],
                std::vec![Image::Closed; SCAN],
                std::vec![Image::Closed; CONN],
                std::vec![Image::Closed; TEST],
            ],
            chains: (0..CONN).map(|_| VecDeque::new()).collect(),
            memory: (0..CONN.saturating_mul(TX))
                .map(|_| (DataPduKind::Continuation, Vec::new()))
                .collect(),
            transmitted: VecDeque::new(),
            nodes: std::vec![Node::Free; nodes],
            scripts: Vec::new(),
            needs_stop: NeedsStop::Quiet,
            alarm: Alarm::Disarmed,
            record: Vec::new(),
            runs: Vec::new(),
            counts: Counts::default(),
        }
    }

    /// The model's clock.
    pub const fn now(&self) -> LeInstant {
        self.now
    }

    /// Move the model's clock to `to`, at the latest instant it reached,
    /// and run what the scheduler serves up to it. While the port needs a
    /// stop, nothing runs.
    pub fn advance(&mut self, to: LeInstant) {
        self.now = self.now.max(to);
        if self.needs_stop != NeedsStop::Quiet {
            return;
        }
        loop {
            let Some(position) = self.list.iter().position(|listed| match listed.run {
                Run::Waiting | Run::Displaced => listed.start <= self.now,
                Run::Begun => listed.end <= self.now,
            }) else {
                return;
            };
            let Some(listed) = self.list.get_mut(position) else {
                return;
            };
            match listed.run {
                Run::Waiting if listed.end > self.now => listed.run = Run::Begun,
                Run::Displaced => {
                    let listed = self.list.remove(position);
                    self.finish(
                        listed.place,
                        EventEnd::NotExecuted(NotExecuted::Displaced),
                        VecDeque::new(),
                    );
                }
                Run::Waiting | Run::Begun => {
                    let listed = self.list.remove(position);
                    self.execute(listed);
                }
            }
        }
    }

    /// Script what the next start does; a start with no script confirms.
    pub fn script_start(&mut self, script: StartScript) {
        self.starts.push_back(script);
    }

    /// Script what the peer of `role`'s `instance` does in its next events,
    /// one turn an event, in the order its events run. An event with no
    /// turn left hears nothing.
    pub fn script(&mut self, role: Role, instance: u8, turn: Turn) {
        match self
            .scripts
            .iter_mut()
            .find(|(id, _)| *id == (role, instance))
        {
            Some((_, turns)) => turns.push_back(turn),
            None => self
                .scripts
                .push(((role, instance), VecDeque::from([turn]))),
        }
    }

    /// Tell the scheduler not to run the listed event `event`: it ends
    /// `NotExecuted(Displaced)` at its start. An event begun runs.
    pub fn displace(&mut self, event: EventId) {
        for listed in &mut self.list {
            if EventId::of(&listed.place) == event && listed.run == Run::Waiting {
                listed.run = Run::Displaced;
            }
        }
    }

    /// Raise an anomaly: the port needs a stop, and gives nothing until it.
    pub fn raise(&mut self, anomaly: Anomaly) {
        if let Phase::Starting(_) | Phase::Running(_) | Phase::Stopping(_) = self.phase
            && self.needs_stop == NeedsStop::Quiet
        {
            self.needs_stop = NeedsStop::Raised(anomaly);
        }
    }

    /// Every run's record, in order; a run ends at its reset or at its
    /// start's failure.
    pub fn runs(&self) -> &[Vec<Programmed>] {
        &self.runs
    }

    /// The model's counts.
    pub const fn counts(&self) -> Counts {
        self.counts
    }

    /// The events listed, not ended.
    pub fn listed(&self) -> usize {
        self.list.len()
    }

    /// The receive nodes the hardware may fill.
    pub fn free_nodes(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| **node == Node::Free)
            .count()
    }

    /// The PDUs in `connection`'s chain.
    pub fn chain(&self, connection: u8) -> usize {
        self.chains
            .get(usize::from(connection))
            .map_or(0, VecDeque::len)
    }

    fn call(&mut self) {
        self.counts.calls = self.counts.calls.saturating_add(1);
    }

    fn since(&self) -> LeInstant {
        match self.phase {
            Phase::Starting(since) | Phase::Running(since) | Phase::Stopping(since) => since,
            Phase::Stopped | Phase::Retired | Phase::Released => self.now,
        }
    }

    fn offset(
        &self,
        at: LeInstant,
    ) -> RadioDuration<oer_protocols_bluetooth_le_contracts_port::LeRadio> {
        at.checked_duration_since(self.since())
            .unwrap_or(RadioDuration::ZERO)
    }

    fn turn(&mut self, event: EventId) -> Option<Turn> {
        let (_, turns) = self
            .scripts
            .iter_mut()
            .find(|(id, _)| *id == (event.role, event.instance))?;
        turns.pop_front()
    }

    fn turn_back(&mut self, event: EventId, turn: Turn) {
        let id = (event.role, event.instance);
        match self.scripts.iter_mut().find(|(key, _)| *key == id) {
            Some((_, turns)) => turns.push_front(turn),
            None => self.scripts.push((id, VecDeque::from([turn]))),
        }
    }

    fn chain_nodes(chain: Chain) -> core::ops::Range<usize> {
        let first = match chain {
            Chain::Connection(instance) => usize::from(instance),
            Chain::Advertising => CONN,
            Chain::Scan => CONN.saturating_add(1),
        }
        .saturating_mul(RX);
        first..first.saturating_add(RX)
    }

    /// Fill a free node of `chain` with `bytes` (header first), or answer
    /// that none is free.
    fn receive(&mut self, chain: Chain, bytes: &[u8], at: LeInstant) -> Option<u16> {
        let range = Self::chain_nodes(chain);
        let position = range
            .clone()
            .find(|&index| self.nodes.get(index) == Some(&Node::Free))?;
        let node = u16::try_from(position).ok()?;
        let (header, payload) = match bytes {
            [first, second, payload @ ..] => ([*first, *second], payload.to_vec()),
            [first] => ([*first, 0], Vec::new()),
            [] => ([0, 0], Vec::new()),
        };
        let slot = self.nodes.get_mut(position)?;
        *slot = Node::Filled(Reception {
            header,
            payload,
            captured_at: Some(at),
        });
        Some(node)
    }

    fn finish(&mut self, place: EventPlace<'id>, end: EventEnd, receptions: VecDeque<u16>) {
        self.finished.push_back(Finished {
            place,
            end,
            receptions,
        });
    }

    fn executed(anchor: Option<LeInstant>, exchanged: u8, peer: Option<MoreData>) -> EventEnd {
        EventEnd::Executed(Executed {
            anchor,
            exchanged,
            cut: Cut::Whole,
            peer,
        })
    }

    /// Run a listed event to its end, with its peer's turn.
    fn execute(&mut self, listed: Listed<'id>) {
        let id = EventId::of(&listed.place);
        let start = listed.start;
        let mut receptions = VecDeque::new();
        let end = match (id.role, self.turn(id)) {
            (Role::Advertising, Some(Turn::Request(pdu))) => {
                match self.receive(Chain::Advertising, &pdu, start) {
                    Some(node) => {
                        receptions.push_back(node);
                        Self::executed(Some(start), listed.sends.saturating_add(1), None)
                    }
                    None => Self::executed(Some(start), listed.sends, None),
                }
            }
            (Role::Scan, Some(Turn::Advertisements(heard))) => {
                let mut exchanged: u8 = 0;
                for pdu in &heard {
                    if let Some(node) = self.receive(Chain::Scan, pdu, start) {
                        receptions.push_back(node);
                        exchanged = exchanged.saturating_add(1);
                    }
                }
                Self::executed(None, exchanged, None)
            }
            (Role::Connection, Some(Turn::Central { heard, ack, sends })) => {
                self.connection_event(id, start, heard, ack, sends, &mut receptions)
            }
            (Role::Test, Some(Turn::Packets(packets))) => Self::executed(None, packets, None),
            (Role::Connection, _) => Self::executed(None, 0, None),
            (Role::Advertising | Role::Test, _) => Self::executed(Some(start), listed.sends, None),
            (Role::Scan, _) => Self::executed(None, 0, None),
        };
        self.finish(listed.place, end, receptions);
    }

    fn connection_event(
        &mut self,
        id: EventId,
        start: LeInstant,
        heard: Heard,
        ack: Ack,
        sends: Vec<u8>,
        receptions: &mut VecDeque<u16>,
    ) -> EventEnd {
        if heard == Heard::Lost {
            // The central's packets are lost: what it sent is sent again.
            if !sends.is_empty() {
                self.turn_back(
                    id,
                    Turn::Central {
                        heard: Heard::Heard,
                        ack: Ack::Withhold,
                        sends,
                    },
                );
            }
            return Self::executed(None, 0, None);
        }
        let mut exchanged: u8 = 1;
        let taken = if sends.is_empty() {
            true
        } else if let Some(node) = self.receive(Chain::Connection(id.instance), &sends, start) {
            receptions.push_back(node);
            exchanged = exchanged.saturating_add(1);
            true
        } else {
            // B6: with no node free the PDU is neither taken nor
            // acknowledged; the central sends it again.
            self.turn_back(
                id,
                Turn::Central {
                    heard: Heard::Heard,
                    ack: Ack::Withhold,
                    sends,
                },
            );
            false
        };
        if ack == Ack::Acknowledge
            && let Some(chain) = self.chains.get_mut(usize::from(id.instance))
            && let Some(place) = chain.pop_front()
        {
            self.transmitted.push_back((TxEnd::Transmitted, place));
            exchanged = exchanged.saturating_add(1);
        }
        let more = if taken {
            MoreData::Done
        } else {
            MoreData::More
        };
        Self::executed(Some(start), exchanged, Some(more))
    }

    /// The blocks' reset: every event out and every PDU in a chain ends.
    fn reset(&mut self) {
        for listed in core::mem::take(&mut self.list) {
            let end = match listed.run {
                Run::Waiting | Run::Displaced => EventEnd::NotExecuted(NotExecuted::Withdrawn),
                Run::Begun => EventEnd::Indeterminate,
            };
            self.finish(listed.place, end, VecDeque::new());
        }
        for chain in &mut self.chains {
            for place in chain.drain(..) {
                self.transmitted.push_back((TxEnd::Withdrawn, place));
            }
        }
        for node in &mut self.nodes {
            if matches!(node, Node::Released) {
                *node = Node::Free;
            }
        }
        self.needs_stop = NeedsStop::Quiet;
        self.alarm = Alarm::Disarmed;
    }

    fn open(&mut self, role: Role, instance: u8) {
        if let Some(image) = self
            .images
            .get_mut(role_index(role))
            .and_then(|images| images.get_mut(usize::from(instance)))
        {
            *image = Image::Open;
        }
        self.record.push(Programmed::Opened(role, instance));
    }

    fn close(&mut self, role: Role, instance: u8) {
        if let Some(image) = self
            .images
            .get_mut(role_index(role))
            .and_then(|images| images.get_mut(usize::from(instance)))
        {
            *image = Image::Closed;
        }
        self.record.push(Programmed::Closed(role, instance));
    }

    fn publish(
        &mut self,
        place: EventPlace<'id>,
        start: LeInstant,
        end: LeInstant,
        sends: u8,
    ) -> Published {
        self.call();
        self.counts.publications = self.counts.publications.saturating_add(1);
        let id = EventId::of(&place);
        let offset = self.offset(start);
        self.record.push(Programmed::Published(id, offset));
        let reservation = start
            .checked_sub(MODEL_CAPABILITIES.timing.preparation_lead)
            .unwrap_or(start);
        let earliest = self.now.checked_add(MODEL_GUARD).unwrap_or(self.now);
        if reservation < earliest {
            self.finish(
                place,
                EventEnd::NotExecuted(NotExecuted::Late),
                VecDeque::new(),
            );
            return Published;
        }
        let position = self
            .list
            .iter()
            .position(|listed| listed.start > start)
            .unwrap_or(self.list.len());
        self.list.insert(
            position,
            Listed {
                place,
                start,
                end,
                run: Run::Waiting,
                sends,
            },
        );
        Published
    }

    fn ending(&self, end: EventEnd) -> Ending {
        match end {
            EventEnd::Executed(Executed {
                anchor,
                exchanged,
                cut,
                peer,
            }) => Ending::Executed {
                anchor: anchor.map(|anchor| self.offset(anchor)),
                exchanged,
                cut,
                peer,
            },
            EventEnd::NotExecuted(why) => Ending::NotExecuted(why),
            EventEnd::Indeterminate => Ending::Indeterminate,
        }
    }

    fn view(node: &Node) -> Received<'_> {
        match node {
            Node::Filled(reception) | Node::Given(reception) | Node::Held(reception) => Received {
                header: reception.header,
                payload: &reception.payload,
                rssi_dbm: -40,
                captured_at: reception.captured_at,
            },
            Node::Free | Node::Released => Received {
                header: [0, 0],
                payload: &[],
                rssi_dbm: 0,
                captured_at: None,
            },
        }
    }
}

const fn role_index(role: Role) -> usize {
    match role {
        Role::Advertising => 0,
        Role::Scan => 1,
        Role::Connection => 2,
        Role::Test => 3,
    }
}

impl<
    'id,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const RX: usize,
    const TEST: usize,
> LeRounds<'id> for ModelPort<'id, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>
{
    type Node = u16;

    fn round_open(&mut self, _: &RoundSeal) -> LeInstant {
        self.call();
        self.counts.rounds = self.counts.rounds.saturating_add(1);
        self.reading.clear();
        self.now
    }

    fn round_next(&mut self, _: &RoundSeal) -> Option<(EventPlace<'id>, EventEnd)> {
        self.call();
        if self.needs_stop != NeedsStop::Quiet {
            return None;
        }
        let finished = self.finished.pop_front()?;
        let id = EventId::of(&finished.place);
        self.record
            .push(Programmed::Ended(id, self.ending(finished.end)));
        self.reading = finished.receptions;
        for &node in &self.reading {
            if let Some(slot) = self.nodes.get_mut(usize::from(node))
                && let Node::Filled(reception) = slot
            {
                *slot = Node::Given(core::mem::replace(
                    reception,
                    Reception {
                        header: [0, 0],
                        payload: Vec::new(),
                        captured_at: None,
                    },
                ));
            }
        }
        Some((finished.place, finished.end))
    }

    fn round_received(&mut self, _: &RoundSeal) -> Option<u16> {
        self.call();
        self.reading.pop_front()
    }

    fn round_view(&self, _: &RoundSeal, node: &u16) -> Received<'_> {
        self.nodes
            .get(usize::from(*node))
            .map_or(Self::view(&Node::Free), Self::view)
    }

    fn round_take(&mut self, _: &RoundSeal, node: u16) -> PduHandle<'id> {
        self.call();
        if let Some(slot) = self.nodes.get_mut(usize::from(node))
            && let Node::Given(reception) = slot
        {
            *slot = Node::Held(core::mem::replace(
                reception,
                Reception {
                    header: [0, 0],
                    payload: Vec::new(),
                    captured_at: None,
                },
            ));
        }
        self.record.push(Programmed::Taken(node));
        self.key.hold(node)
    }

    fn round_held(&self, _: &RoundSeal, handle: &PduHandle<'id>) -> Received<'_> {
        self.nodes
            .get(usize::from(handle.node()))
            .map_or(Self::view(&Node::Free), Self::view)
    }

    fn round_release(&mut self, _: &RoundSeal, handle: PduHandle<'id>) {
        self.call();
        let node = self.key.release_pdu(handle);
        if let Some(slot) = self.nodes.get_mut(usize::from(node)) {
            *slot = Node::Released;
        }
        self.record.push(Programmed::Released(node));
    }

    fn round_transmitted(&mut self, _: &RoundSeal) -> Option<(TxEnd, TxPlace<'id>)> {
        self.call();
        if self.needs_stop != NeedsStop::Quiet {
            return None;
        }
        let (end, place) = self.transmitted.pop_front()?;
        self.record
            .push(Programmed::Transmitted(place.index(), place.slot(), end));
        Some((end, place))
    }

    fn round_close(&mut self, _: &RoundSeal) -> Rest {
        self.call();
        self.reading.clear();
        for node in &mut self.nodes {
            if matches!(node, Node::Given(_) | Node::Released) {
                *node = Node::Free;
            }
        }
        if self.needs_stop != NeedsStop::Quiet {
            return Rest::Quiet;
        }
        if !self.finished.is_empty() || !self.transmitted.is_empty() {
            return Rest::Round;
        }
        self.list
            .iter()
            .map(|listed| match listed.run {
                Run::Waiting | Run::Displaced => listed.start,
                Run::Begun => listed.end,
            })
            .min()
            .map_or(Rest::Quiet, Rest::At)
    }
}

impl<
    'id,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const RX: usize,
    const TEST: usize,
> LePort<'id, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>
    for ModelPort<'id, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>
{
    type Caps = ModelCaps;
    type Released<'a>
        = ()
    where
        Self: 'a;

    fn start(&mut self, stopped: Stopped<'id>) -> Starting<'id> {
        self.call();
        self.phase = Phase::Starting(self.now);
        self.record = std::vec![Programmed::Start];
        self.key.start(stopped)
    }

    fn poll_start(&mut self, starting: Starting<'id>, _: &mut Context<'_>) -> StartPoll<'id> {
        self.call();
        match self.starts.pop_front().unwrap_or(StartScript::Confirm) {
            StartScript::Confirm => {
                self.phase = Phase::Running(self.since());
                self.record.push(Programmed::Started(None));
                StartPoll::Running(self.key.started(starting))
            }
            StartScript::Fail(failure) => {
                // The start took down what it began.
                self.reset();
                self.record.push(Programmed::Started(Some(failure)));
                self.phase = Phase::Stopped;
                self.runs.push(core::mem::take(&mut self.record));
                StartPoll::Stopped(self.key.start_failed(starting), failure)
            }
        }
    }

    fn stop(&mut self, _: &mut StopRight<'id>, from: StopFrom<'id>) -> Stopping<'id> {
        self.call();
        self.phase = Phase::Stopping(self.since());
        self.record.push(Programmed::Stop);
        self.key.stop(from)
    }

    fn poll_stop(&mut self, stopping: Stopping<'id>, _: &mut Context<'_>) -> StopPoll<'id> {
        self.call();
        self.reset();
        self.record.push(Programmed::Reset);
        self.phase = Phase::Stopped;
        self.runs.push(core::mem::take(&mut self.record));
        StopPoll::Stopped(self.key.stopped(stopping))
    }

    fn retire(
        &mut self,
        stopped: Stopped<'id>,
        roles: RoleRights<'id, ADV, SCAN, CONN, TEST>,
    ) -> Retired<'id> {
        self.call();
        self.phase = Phase::Retired;
        self.key.retire(stopped, roles)
    }

    fn release(&mut self, retired: Retired<'id>, right: StopRight<'id>) {
        self.call();
        self.phase = Phase::Released;
        self.key.release(retired, right);
    }

    fn open_advertising(
        &mut self,
        place: AdvPlace<'id>,
        _: &Supported<ModelCaps, AdvertisingSet<'_>>,
    ) -> (Advertiser<'id>, AdvEvent<'id>) {
        self.call();
        self.open(Role::Advertising, place.index());
        self.key.open_advertising(place)
    }

    fn rewrite_advertising(
        &mut self,
        role: &mut Advertiser<'id>,
        _: &mut AdvEvent<'id>,
        _: &Supported<ModelCaps, AdvertisingData<'_>>,
    ) {
        self.call();
        self.record.push(Programmed::Rewritten(role.index()));
    }

    fn close_advertising(&mut self, role: Advertiser<'id>, home: AdvEvent<'id>) -> AdvPlace<'id> {
        self.call();
        self.close(Role::Advertising, role.index());
        self.key.close_advertising(role, home)
    }

    fn open_scanner(
        &mut self,
        place: ScanPlace<'id>,
        _: &Supported<ModelCaps, ScannerSetup>,
    ) -> (Scanner<'id>, ScanEvent<'id>) {
        self.call();
        self.open(Role::Scan, place.index());
        self.key.open_scanner(place)
    }

    fn close_scanner(&mut self, role: Scanner<'id>, home: ScanEvent<'id>) -> ScanPlace<'id> {
        self.call();
        self.close(Role::Scan, role.index());
        self.key.close_scanner(role, home)
    }

    fn open_connection(
        &mut self,
        place: ConnPlace<'id>,
        _: &Supported<ModelCaps, ConnectionSetup>,
    ) -> (
        Connection<'id>,
        [ConnEvent<'id>; EVENTS],
        [TxPlace<'id>; TX],
    ) {
        self.call();
        self.open(Role::Connection, place.index());
        self.key.open_connection(place)
    }

    fn close_connection(
        &mut self,
        role: Connection<'id>,
        events: [ConnEvent<'id>; EVENTS],
        tx: [TxPlace<'id>; TX],
    ) -> ConnPlace<'id> {
        self.call();
        self.close(Role::Connection, role.index());
        self.key.close_connection(role, events, tx)
    }

    fn open_test(
        &mut self,
        place: TestPlace<'id>,
        _: &Supported<ModelCaps, DtmTest>,
    ) -> (Test<'id>, TestEvent<'id>) {
        self.call();
        self.open(Role::Test, place.index());
        self.key.open_test(place)
    }

    fn close_test(&mut self, role: Test<'id>, home: TestEvent<'id>) -> TestPlace<'id> {
        self.call();
        self.close(Role::Test, role.index());
        self.key.close_test(role, home)
    }

    fn write_accept_list(
        &mut self,
        _: &mut AcceptList<'id>,
        entries: &Supported<ModelCaps, AcceptEntries<'_>>,
    ) {
        self.call();
        self.record
            .push(Programmed::AcceptList(entries.get().get().len()));
    }

    fn publish_advertising(
        &mut self,
        place: AdvEvent<'id>,
        event: &Supported<ModelCaps, AdvertisingEvent<ModelCaps>>,
    ) -> Published {
        let event = event.get();
        let channels = event.channels.iter().count();
        let end = event
            .channel_spacing
            .get()
            .checked_mul(u64::try_from(channels).unwrap_or(u64::MAX))
            .and_then(|length| event.anchor.checked_add(length))
            .unwrap_or(event.anchor);
        self.publish(
            EventPlace::Advertising(place),
            event.anchor,
            end,
            u8::try_from(channels).unwrap_or(u8::MAX),
        )
    }

    fn publish_scan(
        &mut self,
        place: ScanEvent<'id>,
        window: &Supported<ModelCaps, ScanWindow>,
    ) -> Published {
        let window = window.get().window;
        self.publish(EventPlace::Scan(place), window.start(), window.end(), 0)
    }

    fn publish_connection(
        &mut self,
        place: ConnEvent<'id>,
        event: &Supported<ModelCaps, ConnectionEvent>,
    ) -> Published {
        let window = event.get().window;
        self.publish(
            EventPlace::Connection(place),
            window.start(),
            window.end(),
            0,
        )
    }

    fn publish_test(
        &mut self,
        place: TestEvent<'id>,
        window: &Supported<ModelCaps, TestWindow>,
    ) -> Published {
        let window = window.get().window;
        self.publish(EventPlace::Test(place), window.start(), window.end(), 1)
    }

    fn withdraw(&mut self, role: RoleRef<'_, 'id>) {
        self.call();
        let (kind, instance) = match role {
            RoleRef::Advertiser(role) => (Role::Advertising, role.index()),
            RoleRef::Scanner(role) => (Role::Scan, role.index()),
            RoleRef::Connection(role) => (Role::Connection, role.index()),
            RoleRef::Test(role) => (Role::Test, role.index()),
        };
        self.record.push(Programmed::Withdrawn(kind, instance));
        let mut kept = Vec::new();
        for listed in core::mem::take(&mut self.list) {
            let id = EventId::of(&listed.place);
            if id.role == kind && id.instance == instance && listed.run != Run::Begun {
                self.finish(
                    listed.place,
                    EventEnd::NotExecuted(NotExecuted::Withdrawn),
                    VecDeque::new(),
                );
            } else {
                kept.push(listed);
            }
        }
        self.list = kept;
        if kind == Role::Connection
            && let Some(chain) = self.chains.get_mut(usize::from(instance))
        {
            for place in chain.drain(..) {
                self.transmitted.push_back((TxEnd::Withdrawn, place));
            }
        }
    }

    fn write_pdu<F: FnOnce(&mut [u8]) -> PduLen<ModelCaps>>(
        &mut self,
        place: &mut TxPlace<'id>,
        llid: DataPduKind,
        fill: F,
    ) {
        self.call();
        self.counts.writes = self.counts.writes.saturating_add(1);
        let mut buffer = [0; MODEL_PAYLOAD as usize];
        let length = usize::from(fill(&mut buffer).get());
        let index = usize::from(place.index())
            .saturating_mul(TX)
            .saturating_add(usize::from(place.slot()));
        if let Some(memory) = self.memory.get_mut(index) {
            *memory = (llid, buffer.get(..length).unwrap_or(&[]).to_vec());
        }
    }

    fn submit(&mut self, place: TxPlace<'id>) {
        self.call();
        self.counts.submissions = self.counts.submissions.saturating_add(1);
        let index = usize::from(place.index())
            .saturating_mul(TX)
            .saturating_add(usize::from(place.slot()));
        let (llid, payload) = self
            .memory
            .get(index)
            .cloned()
            .unwrap_or((DataPduKind::Continuation, Vec::new()));
        self.record.push(Programmed::Submitted {
            instance: place.index(),
            slot: place.slot(),
            llid,
            payload,
        });
        if let Some(chain) = self.chains.get_mut(usize::from(place.index())) {
            chain.push_back(place);
        }
    }

    fn alarm(&mut self, at: Option<LeInstant>) {
        self.call();
        self.alarm = at.map_or(Alarm::Disarmed, Alarm::At);
    }

    fn poll_facts(&mut self, _: &mut Context<'_>) -> Facts {
        self.call();
        let quiet = self.needs_stop == NeedsStop::Quiet;
        let finished = !self.finished.is_empty() || !self.transmitted.is_empty();
        Facts {
            finished: if quiet && finished {
                Signal::Raised
            } else {
                Signal::Quiet
            },
            alarm: match self.alarm {
                Alarm::At(at) if at <= self.now => Signal::Raised,
                Alarm::At(_) | Alarm::Disarmed => Signal::Quiet,
            },
            needs_stop: if quiet { Signal::Quiet } else { Signal::Raised },
        }
    }
}
