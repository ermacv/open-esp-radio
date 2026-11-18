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
#![cfg_attr(
    test,
    allow(
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
        clippy::disallowed_methods,
    )
)]

//! A model of hardware behind the port's contract
//! (`design/port-contract-2026-11-16.md` of the notes repository), the
//! subject of the conformance suite (`oer-protocols-ieee80211-testing-conformance`).
//!
//! [`ModelPort`] stands where a chip's port stands: it implements the
//! radio's life ([`RadioPort`]), the data path ([`DataPort`]) and the
//! receive round's primitives ([`ReceivePort`]) over a modelled MAC:
//!
//! - a ring of descriptors a test lands units on ([`ModelPort::land`]) and
//!   staging places a round stages them into;
//! - four transmit queues and two arenas, with an air a test sets for each
//!   queue: it acknowledges, does not, times out or denies ([`Air`]);
//! - a MAC a test can tell not to go idle ([`ModelPort::hold_busy`]), so
//!   that the stop's last resort runs: its block reset, then the
//!   programming of the owner's taking again;
//! - each anomaly a backend may indicate, raisable at any point of a run
//!   ([`ModelPort::raise`]);
//! - a record of what it was asked to program in a run
//!   ([`ModelPort::record`]), for "a second run is as the first";
//! - counts of its calls and of the ring's services ([`ModelPort::counts`]).
//!
//! It holds state about its modelled hardware only (rule 7 of the
//! contract): the radio's phase, the ring's and the queues' positions,
//! what is on the air, its latched facts and its counts.

extern crate std;

use core::cell::Cell;
use core::num::NonZeroU16;
use core::task::{Context, Poll};
use std::collections::VecDeque;
use std::rc::Rc;
use std::vec::Vec;

use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::data::Sealed;
use oer_protocols_ieee80211_contracts_port::{
    AggregateEnd, AgreementCommand, AgreementEnd, AgreementHeld, Air as OnAirAir,
    AmpduCapabilities, AmpduLengthTable, Arena, ArenaOutcome, ArenaSide, BandSet, BlockAckReport,
    Channel, CloseGate, CommandPlace, CommandPoll, Complete, DataLayout, DataPort, DataRights,
    Ended, EthernetAddresses, Facts, FrameSet, GateClosed, GateIssuer, GateLent, GateOpen,
    GroupCommand, GroupEnd, GroupHeld, HEADROOM, HardwareServices, HeaderLen, IdleArena,
    Ieee80211Capabilities, Ieee80211Instant, InCommand, IndeterminateCause, Lendable,
    LowerMacCapabilities, MpduLength, OnAir, OutsideTheUnit, PairwiseCommand, PairwiseEnd,
    PairwiseHeld, PhyFormatSet, Place, PortKey, PortSink, Ppdu, Protection, QueueRight, RadioPort,
    RateSupport, ReceiveFilter, ReceivePort, Rest, Retired, RetunePoll, Retuning, Running, RxHead,
    RxMeta, Settled, Signal, SingleEnd, SingleOutcome, SinglePlace, SingleResponse, Staged,
    StagedPlaces, StartFailure, StartPoll, Starting, StopFrom, StopPoll, StopRight, Stopped,
    Stopping, SupportedChannel, Tid, TxFacts, TxFrame, TxOutcome, TxSlot, TxSlotOf, UnitsHome,
    ValueCommand, ValueEnd, ValueKind, WidthSet, WithdrawCause,
};
use oer_protocols_ieee80211_wire::phy::{HeMcs, HtMcs, LegacyRate, PhyRate};
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;
use oer_protocols_radio_port::ClockError;

/// The longest MPDU the model carries.
pub const MODEL_MAX_MPDU: usize = 2304;

/// The A-MPDU length the model sends at every HT and HE rate but one.
pub const MODEL_AMPDU_LENGTH: u32 = 6_500;

/// The one rate the model aggregates less at: HT MCS 7 at 20 MHz with the
/// long guard interval, where one frame of the longest MPDU fits and two
/// do not.
pub const MODEL_SHORT_AMPDU_MCS: u8 = 7;
// The table's entry below is the eighth MCS: the two say the same rate.
const _: () = if MODEL_SHORT_AMPDU_MCS != 7 {
    panic!("the short entry of the table is MCS 7")
};

/// The A-MPDU length at [`MODEL_SHORT_AMPDU_MCS`].
pub const MODEL_SHORT_AMPDU_LENGTH: u32 = 3_000;

/// The model's A-MPDU lengths.
pub static MODEL_AMPDU_LENGTHS: AmpduLengthTable = {
    let mut ht = [[[MODEL_AMPDU_LENGTH; 32]; 2]; 2];
    let [[[_, _, _, _, _, _, _, seven, ..], _], _] = &mut ht;
    *seven = MODEL_SHORT_AMPDU_LENGTH;
    AmpduLengthTable {
        ht,
        he: [[[MODEL_AMPDU_LENGTH; 12]; 3]; 4],
    }
};

/// What the model serves.
pub const MODEL_CAPABILITIES: LowerMacCapabilities = LowerMacCapabilities {
    bands: BandSet::GHZ2_4.union(BandSet::GHZ5),
    formats: oer_protocols_ieee80211_contracts_port::FormatWidths {
        non_ht: WidthSet::MHZ20,
        ht: WidthSet::MHZ20.union(WidthSet::MHZ40),
        he: WidthSet::MHZ20,
    },
    rates: RateSupport {
        dsss_cck: oer_protocols_ieee80211_contracts_port::Support::Supported,
        ofdm: oer_protocols_ieee80211_contracts_port::Support::Supported,
        ht_max_mcs: HtMcs::new(7),
        he_max_mcs: HeMcs::new(9),

        he_dcm: oer_protocols_ieee80211_contracts_port::Support::Supported,
        he_ldpc: oer_protocols_ieee80211_contracts_port::Support::Supported,
        spatial_streams: 1,
    },
    services: HardwareServices::FCS
        .union(HardwareServices::IMMEDIATE_ACK)
        .union(HardwareServices::CIPHER_TRANSFORM)
        .union(HardwareServices::RX_BLOCK_ACK_MATCHING),
    vifs: 2,
    tx_queues: 4,
    max_mpdu_length: MODEL_MAX_MPDU as u16,
    tx_power_ceiling_min_dbm: Some(0),
    individual_no_ack: PhyFormatSet::NON_HT,
    station_receive_filters: ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
    access_point_receive_filters: ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::PROBE_REQUESTS),
    rx_block_ack_max_tid: 7,
    rx_block_ack_max_window: 64,
    rx_amsdu_in_ampdu: true,
    edca_max_aifsn: 15,
    edca_max_cw_exponent: 10,
    edca_max_txop_limit_units_32_us: u16::MAX,
};

/// The model's capabilities marker.
pub struct ModelCaps;

impl Ieee80211Capabilities for ModelCaps {
    type SppScope = oer_protocols_ieee80211_wire::security::spp::Interface;
    const CAPS: LowerMacCapabilities = MODEL_CAPABILITIES;
}

/// The subframes of one arena.
pub const MODEL_ARENA_SLOTS: u8 = 32;
/// The model's staging places.
pub const MODEL_PLACES: usize = 8;
/// The model's octets in front of a transmit slot's headroom: not the
/// S31's 8, so that a chip's number written where the port's belongs fails
/// on the model.
pub const MODEL_TX_FRONT: usize = 12;
/// The model's octets behind a body and its MIC's room: not the S31's 4.
pub const MODEL_TX_BEHIND: usize = 8;
/// The longest body the model's transmit slots hold: the slots' owner's
/// number (the suite sends bodies of 2 000).
pub const MODEL_TX_BODY: usize = 2_048;
/// The octets of a model transmit slot.
pub const MODEL_TX_SLOT: usize = 2_128;
/// What the model writes in its own octets in front of the headroom.
const FRONT_OCTET: u8 = 0xf5;
/// What the model writes in its own octets behind a body and its MIC.
const BEHIND_OCTET: u8 = 0xb5;
/// The descriptors of the model's ring.
pub const MODEL_RING: usize = 16;
/// The station's group key slots.
pub const MODEL_GROUPS: usize = 2;
/// The receive agreements' slots.
pub const MODEL_AGREEMENTS: usize = 2;

/// The layout of the model's data's rights, with `SINGLES` single places.
pub type ModelLayout<const SINGLES: usize, const DATA: usize> =
    DataLayout<SINGLES, MODEL_GROUPS, MODEL_AGREEMENTS, DATA>;

/// The model's data's rights.
pub type ModelRights<'id, const SINGLES: usize, const DATA: usize> =
    DataRights<'id, SINGLES, MODEL_GROUPS, MODEL_AGREEMENTS, DATA>;

/// The network's memory of one MSDU, by its handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelMemory {
    /// The network's name of the memory.
    pub id: u32,
    /// The body's length.
    pub len: u16,
    /// The octets the network wrote in front of the body: a data MSDU's
    /// Ethernet header at their end.
    pub front: [u8; HEADROOM],
}

/// One publication of an arena, as the model put it on the air.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Publication {
    /// One MPDU went, not an A-MPDU.
    pub single: Went,
    /// The rate it went at.
    pub rate: PhyRate,
    /// The protection it went behind: an aggregate's, or each stage's of a
    /// single's plan.
    pub protection: Vec<Protection>,
    /// The MPDUs' fronts as they went, the Retry bit set on a frame that
    /// went before.
    pub frames: Vec<Vec<u8>>,
}

/// The link an arena is opened for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelLink {
    /// Whether the link's frames are protected.
    pub protected: bool,
}

/// What the air does with the next frame of a queue.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Air {
    /// The receiver acknowledges it.
    #[default]
    Ack,
    /// The receiver does not.
    NoAck,
    /// The queue times out: sent or not is not known.
    Timeout,
    /// The air denies it.
    Deny,
    /// It stays on the air until the air is set again.
    Hold,
}

/// An anomaly a backend may indicate: the place that saw it gives nothing
/// until the stop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Anomaly {
    /// The MAC's fatal interrupt.
    MacFatal,
    /// A completion status outside the known ones.
    CompletionStatus,
    /// The receive walker's last descriptor absent while the ring is live.
    WalkerLast,
    /// A published frame whose completion does not come after its queue was
    /// disabled.
    NoCompletion,
}

/// What the model was asked to program, in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Programmed {
    /// The channel a start or a retune tuned.
    Channel(Channel),
    /// The interrupts unmasked or masked.
    Interrupts(bool),
    /// The transmit queues enabled or disabled.
    Queues(bool),
    /// The receive walker started or stopped.
    Walker(bool),
    /// The MAC's runtime started or stopped.
    MacRuntime(bool),
    /// The gate opened or closed.
    Gate(bool),
    /// The pairwise key installed or cleared.
    Pairwise(bool),
    /// A group key slot installed or cleared.
    Group(u8, bool),
    /// A receive agreement begun or ended.
    Agreement(u8, bool),
    /// The station's receive configured.
    Configure,
    /// The EDCA parameters set.
    Edca,
    /// The BSS color set.
    Color,
    /// The station's HE state set.
    He,
    /// The MAC's block reset of the stop's last resort.
    BlockReset,
    /// The owner's taking of the block, programmed again after its reset.
    OwnerTaking,
}

/// The model's counts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counts {
    /// The port's calls.
    pub calls: u32,
    /// The ring's services.
    pub services: u32,
    /// The rounds opened.
    pub rounds: u32,
    /// The rounds closed.
    pub closes: u32,
    /// The headers decoded.
    pub decodes: u32,
    /// The units neither a data, a management nor a control frame: given
    /// back to the ring at their staging.
    pub dropped: u32,
    /// The checks that found the model's own octets of a transmit slot
    /// (in front of the headroom, behind the body and its MIC) not as it
    /// wrote them, after a write or at a publication.
    pub overwritten: u32,
}

/// A part of the modelled hardware that is on or off, or one of the
/// model's own levers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Part {
    /// The interrupts are unmasked.
    Interrupts,
    /// The queues are enabled.
    Queues,
    /// The receive walker runs.
    Walker,
    /// The MAC's runtime runs.
    MacRuntime,
    /// The gate is open.
    Gate,
    /// The pairwise key is installed.
    Pairwise,
    /// The port needs a stop.
    NeedsStop,
    /// The lever: the next stop's busy bits hold, so its last resort runs.
    Busy,
    /// The lever: the next command expires.
    Expire,
}

/// A key or agreement slot of the modelled hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Slot {
    Held,
    Empty,
}

/// The parts that are on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Parts(u16);

impl Parts {
    const LEVERS: u16 = Self::bit(Part::Busy) | Self::bit(Part::Expire);

    const fn bit(part: Part) -> u16 {
        1 << part as u16
    }

    /// Whether `part` is on.
    pub const fn has(self, part: Part) -> bool {
        self.0 & Self::bit(part) != 0
    }

    const fn on(&mut self, part: Part) {
        self.0 |= Self::bit(part);
    }

    const fn off(&mut self, part: Part) {
        self.0 &= !Self::bit(part);
    }

    /// Whether `part` was on; it is off after.
    const fn take(&mut self, part: Part) -> bool {
        let was = self.has(part);
        self.off(part);
        was
    }

    /// The hardware's parts, without the model's levers.
    const fn held(self) -> Self {
        Self(self.0 & !Self::LEVERS)
    }
}

/// The places of the mask `bits`, in order.
fn places_of(bits: u64) -> Vec<u8> {
    (0..64_u8)
        .filter(|place| bits & 1_u64.checked_shl(u32::from(*place)).unwrap_or(0) != 0)
        .collect()
}

/// What the modelled hardware holds that a stop must take down: compared
/// before a start and after its stop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardwareState {
    /// The parts of the hardware that are on.
    pub parts: Parts,
    /// The group key slots installed.
    pub groups: [bool; MODEL_GROUPS],
    /// The agreements running.
    pub agreements: [bool; MODEL_AGREEMENTS],
    /// A frame is on the air.
    pub on_air: bool,
    /// Units the network holds.
    pub lent: usize,
}

/// The radio's phase, as the MAC is in it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Stopped,
    Starting,
    Running,
    Retuning,
    Stopping,
}

/// One frame of an arena.
#[derive(Clone, Debug)]
struct Frame {
    memory: ModelMemory,
    header: u8,
    /// The headroom after the engine wrote the front.
    front: [u8; HEADROOM],
    /// The frame's transmit slot: the model's octets in front, the
    /// headroom, the body, its MIC's room, the model's octets behind.
    slot: Vec<u8>,
    /// The publications it went in.
    attempts: u8,
}

impl Frame {
    /// The MPDU's front as it goes on the air: the Retry bit set when the
    /// frame went before (IEEE Std 802.11-2020, 9.2.4.1.7).
    fn on_air(&self) -> Vec<u8> {
        let mut front: Vec<u8> = self
            .front
            .iter()
            .skip(HEADROOM.saturating_sub(usize::from(self.header)))
            .copied()
            .collect();
        if self.attempts > 0
            && let Some(control) = front.get_mut(1)
        {
            // Retry is bit 11 of Frame Control: bit 3 of its second octet.
            *control |= 0x08;
        }
        front
    }
}

/// One arena of the model.
#[derive(Debug, Default)]
struct ArenaBuffer {
    frames: Vec<Frame>,
    sent: u8,
    went: Went,
    /// The rate the publication went at.
    rate: Option<PhyRate>,
    air: Option<Air>,
    ended: Option<Ended<(), (), ()>>,
}

/// What an arena's publication put on the air.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Went {
    /// One MPDU.
    #[default]
    Single,
    /// An A-MPDU.
    Aggregate,
}

/// A queue of the model: the single on its air.
#[derive(Clone, Copy, Debug, Default)]
struct Queue {
    single: Option<u8>,
    air: Air,
    rate: Option<PhyRate>,
    /// The stop disabled the queue with its single on it.
    withdrawn: bool,
}

/// A staged unit in its place.
#[derive(Debug)]
struct Unit {
    staged: Staged<RxMeta>,
    bytes: Vec<u8>,
    lent: bool,
}

/// Whether a command runs on the model's one command place.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum Commanding {
    Idle,
    Running,
}

/// A unit lent to the network: its drop returns it to the model's ring.
#[derive(Debug)]
pub struct ModelLent {
    place: u8,
    returns: Rc<Cell<u64>>,
}

impl ModelLent {
    /// The unit's place.
    pub const fn place(&self) -> u8 {
        self.place
    }
}

impl Drop for ModelLent {
    fn drop(&mut self) {
        let place = 1_u64.checked_shl(u32::from(self.place)).unwrap_or(0);
        self.returns.set(self.returns.get() | place);
    }
}

/// The model port of one installation, with `SINGLES` single places of
/// its controller and `DATA` of an access point's data, staging received
/// units into `PLACES` places ([`MODEL_PLACES`] unless a search names
/// fewer).
pub struct ModelPort<
    'id,
    const SINGLES: usize,
    const DATA: usize,
    const PLACES: usize = MODEL_PLACES,
    const TX_FRONT: usize = MODEL_TX_FRONT,
> {
    key: PortKey<'id>,
    issuer: GateIssuer<'id>,
    phase: Phase,
    channel: SupportedChannel<ModelCaps>,
    start_failure: Option<StartFailure>,
    parts: Parts,
    groups: [bool; MODEL_GROUPS],
    agreements: [bool; MODEL_AGREEMENTS],
    anomaly: Option<Anomaly>,
    arenas: [ArenaBuffer; 2],
    queues: [Queue; 4],
    singles: Vec<Vec<u8>>,
    ring: VecDeque<(Vec<u8>, RxMeta)>,
    places: Vec<Option<Unit>>,
    /// The most units one service of the ring stages.
    frontier: usize,
    returns: Rc<Cell<u64>>,
    /// The frames the next aggregate's BlockAck acknowledges.
    acknowledge: Option<FrameSet>,
    /// The most frames the next arena's publication sends: the prefix its
    /// rate admits, as a test sets it.
    prefix: Option<u8>,
    /// The next command's poll serves the singles' ends first, through the
    /// sink it is lent, as a port does while its command waits at its
    /// barrier.
    serve_in_poll: bool,
    /// Every arena's publication, in order.
    published: Vec<Publication>,
    /// Each memory's last written front, by its id.
    fronts: Vec<(u32, Vec<u8>)>,
    commanding: Commanding,
    record: Vec<Programmed>,
    counts: Counts,
    /// The transmit slots' geometry, checked at build.
    tx: TxSlot<Self>,
}

const fn queue_index(category: oer_protocols_ieee80211_wire::qos::WmmAccessCategory) -> usize {
    use oer_protocols_ieee80211_wire::qos::WmmAccessCategory as Ac;
    match category {
        Ac::Voice => 0,
        Ac::Video => 1,
        Ac::BestEffort => 2,
        Ac::Background => 3,
    }
}

/// A transmit slot as the model lays it out for `memory`: its own octets
/// in front of the headroom and behind the body and its MIC's room, the
/// network's front in the headroom, the body's octets (the memory's id),
/// the MIC's room empty. A body longer than the slot's runs into the
/// model's octets behind: [`whole`] finds them overwritten.
fn lay_out<P>(tx: TxSlot<P>, memory: &ModelMemory) -> Vec<u8> {
    let body = usize::from(memory.len);
    let mut slot = std::vec![0; tx.len().max(tx.body_at().saturating_add(body))];
    if let Some(front) = slot.get_mut(tx.front()) {
        front.fill(FRONT_OCTET);
    }
    if let Some(behind) = tx.behind(body)
        && let Some(octets) = slot.get_mut(behind)
    {
        octets.fill(BEHIND_OCTET);
    }
    let headroom = tx.headroom_at();
    if let Some(room) = slot.get_mut(headroom..headroom.saturating_add(HEADROOM)) {
        for (to, from) in room.iter_mut().zip(memory.front) {
            *to = from;
        }
    }
    let tag = memory.id.to_le_bytes()[0];
    if let Some(octets) = slot.get_mut(tx.body_at()..tx.body_at().saturating_add(body)) {
        octets.fill(tag);
    }
    slot
}

/// Whether a slot's octets in front of the headroom and behind a body of
/// `body` octets and its MIC's room are the model's, as [`lay_out`] wrote
/// them.
fn whole<P>(tx: TxSlot<P>, slot: &[u8], body: u16) -> bool {
    let front = slot
        .get(tx.front())
        .is_some_and(|octets| octets.iter().all(|octet| *octet == FRONT_OCTET));
    let behind = tx.behind(usize::from(body)).is_some_and(|range| {
        slot.get(range)
            .is_some_and(|octets| octets.iter().all(|octet| *octet == BEHIND_OCTET))
    });
    front && behind
}

const fn side_index(side: ArenaSide) -> usize {
    match side {
        ArenaSide::First => 0,
        ArenaSide::Second => 1,
    }
}

impl<'id, const SINGLES: usize, const DATA: usize, const PLACES: usize, const TX_FRONT: usize>
    ModelPort<'id, SINGLES, DATA, PLACES, TX_FRONT>
{
    /// The model of one installation, from what its mint gave the port, on
    /// `channel` before its first start.
    pub fn new(
        key: PortKey<'id>,
        issuer: GateIssuer<'id>,
        channel: SupportedChannel<ModelCaps>,
    ) -> Self {
        Self {
            key,
            issuer,
            phase: Phase::Stopped,
            channel,
            start_failure: None,
            parts: Parts(0),
            groups: [false; MODEL_GROUPS],
            agreements: [false; MODEL_AGREEMENTS],
            anomaly: None,
            arenas: [ArenaBuffer::default(), ArenaBuffer::default()],
            queues: [Queue::default(); 4],
            singles: (0..SINGLES.saturating_add(DATA))
                .map(|_| Vec::new())
                .collect(),
            ring: VecDeque::new(),
            places: (0..PLACES).map(|_| None).collect(),
            frontier: PLACES,
            returns: Rc::new(Cell::new(0)),
            acknowledge: None,
            prefix: None,
            serve_in_poll: false,
            published: Vec::new(),
            fronts: Vec::new(),
            commanding: Commanding::Idle,
            record: Vec::new(),
            counts: Counts::default(),
            tx: TxSlotOf::<'id, Self, MODEL_TX_BODY, MODEL_TX_SLOT>::SLOT,
        }
    }

    fn call(&mut self) {
        self.counts.calls = self.counts.calls.saturating_add(1);
    }

    fn program(&mut self, programmed: Programmed) {
        self.record.push(programmed);
    }

    /// Stage at most `frontier` units a service of the ring, as a ring
    /// whose service is bounded.
    pub fn set_frontier(&mut self, frontier: usize) {
        self.frontier = frontier;
    }

    /// The places that hold a unit, in order.
    pub fn held_places(&self) -> Vec<u8> {
        (0_u8..)
            .zip(&self.places)
            .filter_map(|(index, place)| place.as_ref().map(|_| index))
            .collect()
    }

    /// The received MPDUs the ring still holds.
    pub fn ring_len(&self) -> usize {
        self.ring.len()
    }

    /// The staging places that hold a unit: staged, taken or lent.
    pub fn places_held(&self) -> usize {
        self.places.iter().filter(|place| place.is_some()).count()
    }

    /// The bytes of the unit at `place`, as the port holds them.
    pub fn place_bytes(&self, place: u8) -> Option<&[u8]> {
        self.places
            .get(usize::from(place))?
            .as_ref()
            .map(|unit| unit.bytes.as_slice())
    }

    /// Land a received MPDU on the ring.
    pub fn land(&mut self, bytes: &[u8], meta: RxMeta) {
        if self.ring.len() < MODEL_RING {
            self.ring.push_back((bytes.to_vec(), meta));
        }
    }

    /// Set the air of a queue for its next frames; an arena goes on the
    /// best effort queue's air.
    pub fn set_air(
        &mut self,
        category: oer_protocols_ieee80211_wire::qos::WmmAccessCategory,
        air: Air,
    ) {
        if let Some(queue) = self.queues.get_mut(queue_index(category)) {
            queue.air = air;
        }
    }

    /// Make the next start or retune fail with `failure`.
    pub const fn fail_start(&mut self, failure: StartFailure) {
        self.start_failure = Some(failure);
    }

    /// Keep the MAC busy: the next stop's last resort runs.
    pub const fn hold_busy(&mut self) {
        self.parts.on(Part::Busy);
    }

    /// Make the next key or agreement command expire at its bound.
    pub const fn expire_next(&mut self) {
        self.parts.on(Part::Expire);
    }

    /// The hardware sees `anomaly`: the port needs a stop.
    pub fn raise(&mut self, anomaly: Anomaly) {
        if self.phase != Phase::Stopped {
            self.parts.on(Part::NeedsStop);
            self.anomaly = Some(anomaly);
        }
    }

    /// Let the next command's poll serve the singles' ends first.
    pub const fn serve_in_next_poll(&mut self) {
        self.serve_in_poll = true;
    }

    /// Let the next arena's publication send at most `frames` of its
    /// frames, one at least: the prefix its rate admits.
    pub const fn send_at_most(&mut self, frames: u8) {
        self.prefix = Some(frames);
    }

    /// Let the next aggregate's BlockAck acknowledge only the frames of
    /// `frames`, by their positions.
    pub const fn acknowledge(&mut self, frames: FrameSet) {
        self.acknowledge = Some(frames);
    }

    /// Every arena's publication, in order.
    pub fn published(&self) -> &[Publication] {
        &self.published
    }

    /// The front the engine last wrote in front of the memory `id`.
    pub fn front(&self, id: u32) -> Option<&[u8]> {
        self.fronts
            .iter()
            .find(|(written, _)| *written == id)
            .map(|(_, front)| front.as_slice())
    }

    /// What the model was asked to program since the last start.
    pub fn record(&self) -> &[Programmed] {
        &self.record
    }

    /// The model's counts.
    pub const fn counts(&self) -> Counts {
        self.counts
    }

    /// What the modelled hardware holds.
    pub fn state(&self) -> HardwareState {
        HardwareState {
            parts: self.parts.held(),
            groups: self.groups,
            agreements: self.agreements,
            on_air: self.queues.iter().any(|queue| queue.single.is_some())
                || self.arenas.iter().any(|arena| arena.air.is_some()),
            lent: self
                .places
                .iter()
                .flatten()
                .filter(|unit| unit.lent)
                .count(),
        }
    }

    /// The bytes written in a single's place.
    pub fn single(&self, place: u8) -> Option<&[u8]> {
        self.singles.get(usize::from(place)).map(Vec::as_slice)
    }

    /// The modelled hardware's whole state, for a search that merges equal
    /// states: its phase, parts, slots, arenas, queues, singles' places,
    /// command and levers. Its records and counts are history, not state,
    /// and are left out.
    pub fn state_key(&self) -> std::string::String {
        std::format!(
            "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
            self.receive_key(),
            self.phase,
            self.parts,
            self.groups,
            self.agreements,
            self.anomaly,
            self.arenas,
            self.queues,
            self.singles,
            self.commanding,
            self.acknowledge,
            self.prefix,
        )
    }

    /// The receive side's state: the ring's frames, each place's unit and
    /// whether it is lent, and the returns not yet taken.
    fn receive_key(&self) -> std::string::String {
        let places: Vec<Option<(&[u8], bool)>> = self
            .places
            .iter()
            .map(|place| {
                place
                    .as_ref()
                    .map(|unit| (unit.bytes.as_slice(), unit.lent))
            })
            .collect();
        let ring: Vec<&[u8]> = self
            .ring
            .iter()
            .map(|(bytes, _)| bytes.as_slice())
            .collect();
        std::format!("{ring:?}|{places:?}|{}", self.returns.get())
    }

    /// The arena on the air.
    pub fn on_air_side(&self) -> Option<ArenaSide> {
        [ArenaSide::First, ArenaSide::Second]
            .into_iter()
            .find(|side| {
                self.arenas
                    .get(side_index(*side))
                    .is_some_and(|arena| arena.air.is_some())
            })
    }

    /// The network's names of the memories an arena holds, in its order.
    pub fn arena_memories(&self, side: ArenaSide) -> Vec<u32> {
        self.arenas
            .get(side_index(side))
            .map(|arena| arena.frames.iter().map(|frame| frame.memory.id).collect())
            .unwrap_or_default()
    }

    /// The frames an arena holds.
    pub fn arena_frames(&self, side: ArenaSide) -> usize {
        self.arenas
            .get(side_index(side))
            .map_or(0, |arena| arena.frames.len())
    }

    /// The start's programming, in order.
    fn program_start(&mut self) {
        self.parts.on(Part::Interrupts);
        self.program(Programmed::Interrupts(true));
        self.parts.on(Part::Queues);
        self.program(Programmed::Queues(true));
        self.parts.on(Part::Walker);
        self.program(Programmed::Walker(true));
        self.parts.on(Part::MacRuntime);
        self.program(Programmed::MacRuntime(true));
    }

    /// The stop's taking down, in the start's order reversed.
    fn take_down(&mut self) {
        self.parts.off(Part::MacRuntime);
        self.program(Programmed::MacRuntime(false));
        self.parts.off(Part::Walker);
        self.program(Programmed::Walker(false));
        self.parts.off(Part::Queues);
        self.program(Programmed::Queues(false));
        self.parts.off(Part::Interrupts);
        self.program(Programmed::Interrupts(false));
    }

    /// Withdraw what is queued and end what is on the air, as a stop does.
    fn withdraw_air(&mut self) {
        for arena in &mut self.arenas {
            if arena.air.take().is_some() && arena.ended.is_none() {
                let cause = WithdrawCause::Disabled;
                arena.ended = Some(Ended {
                    outcome: match arena.went {
                        Went::Single => ArenaOutcome::Single(TxOutcome::Withdrawn(cause)),
                        Went::Aggregate => ArenaOutcome::Aggregate(TxOutcome::Withdrawn(cause)),
                    },
                    sent: FrameSet::prefix(arena.sent),
                    acknowledged: FrameSet::EMPTY,
                });
            }
        }
        for queue in &mut self.queues {
            if queue.single.is_some() {
                queue.withdrawn = true;
            }
        }
    }

    fn air_of(&self, category: oer_protocols_ieee80211_wire::qos::WmmAccessCategory) -> Air {
        self.queues
            .get(queue_index(category))
            .map_or(Air::Ack, |queue| queue.air)
    }

    fn rate_air(rate: PhyRate) -> OnAirAir {
        OnAirAir {
            transmissions: NonZeroU16::MIN,
            rate,
        }
    }

    fn free_places(&self) -> usize {
        self.places.iter().filter(|place| place.is_none()).count()
    }
}

impl<'id, const SINGLES: usize, const DATA: usize, const PLACES: usize, const TX_FRONT: usize>
    RadioPort<'id> for ModelPort<'id, SINGLES, DATA, PLACES, TX_FRONT>
{
    type Caps = ModelCaps;
    type Channel = SupportedChannel<ModelCaps>;
    type Data = ModelRights<'id, SINGLES, DATA>;
    type Released<'a>
        = ()
    where
        Self: 'a;

    fn start(
        &mut self,
        stopped: Stopped<'id>,
        channel: Self::Channel,
        _now: Instant,
    ) -> Starting<'id> {
        self.call();
        self.record.clear();
        self.phase = Phase::Starting;
        self.channel = channel;
        self.program(Programmed::Channel(channel.channel()));
        self.program_start();
        self.key.start(stopped)
    }

    fn poll_start(
        &mut self,
        starting: Starting<'id>,
        _cx: &mut Context<'_>,
        _now: Instant,
    ) -> StartPoll<'id> {
        self.call();
        if let Some(failure) = self.start_failure.take() {
            self.take_down();
            self.phase = Phase::Stopped;
            return StartPoll::Stopped(self.key.start_failed(starting), failure);
        }
        self.phase = Phase::Running;
        StartPoll::Running(self.key.started(starting))
    }

    fn retune(
        &mut self,
        running: Running<'id>,
        channel: Self::Channel,
        _gate: GateLent<'_, 'id>,
        _now: Instant,
    ) -> Retuning<'id> {
        self.call();
        self.phase = Phase::Retuning;
        self.channel = channel;
        self.program(Programmed::Channel(channel.channel()));
        self.key.retune(running)
    }

    fn poll_retune(
        &mut self,
        retuning: Retuning<'id>,
        _cx: &mut Context<'_>,
        _now: Instant,
    ) -> RetunePoll<'id> {
        self.call();
        if let Some(failure) = self.start_failure.take() {
            self.take_down();
            self.phase = Phase::Stopped;
            return RetunePoll::Stopped(self.key.retune_failed(retuning), failure);
        }
        self.phase = Phase::Running;
        RetunePoll::Running(self.key.retuned(retuning))
    }

    fn stop(&mut self, _right: &mut StopRight<'id>, from: StopFrom<'id>) -> Stopping<'id> {
        self.call();
        self.phase = Phase::Stopping;
        self.key.stop(from)
    }

    fn poll_stop(
        &mut self,
        stopping: Stopping<'id>,
        _cx: &mut Context<'_>,
        _now: Instant,
    ) -> StopPoll<'id> {
        self.call();
        self.withdraw_air();
        self.take_down();
        if self.parts.take(Part::Busy) {
            // The MAC did not go idle within the bound: its own block's
            // reset clears what the owner programmed when it took it, the
            // key table included, and the stop runs that programming again.
            self.program(Programmed::BlockReset);
            self.parts.off(Part::Pairwise);
            self.groups = [false; MODEL_GROUPS];
            self.agreements = [false; MODEL_AGREEMENTS];
            self.program(Programmed::OwnerTaking);
        }
        self.parts.off(Part::NeedsStop);
        self.anomaly = None;
        self.phase = Phase::Stopped;
        StopPoll::Stopped(self.key.stopped(stopping))
    }

    fn poll_units_home(&mut self, _cx: &mut Context<'_>) -> Poll<UnitsHome<'id>> {
        self.call();
        let returned = places_of(self.returns.replace(0));
        for place in returned {
            if let Some(slot) = self.places.get_mut(usize::from(place)) {
                *slot = None;
            }
        }
        if self.places.iter().flatten().any(|unit| unit.lent) {
            Poll::Pending
        } else {
            Poll::Ready(self.key.units_home())
        }
    }

    fn retire(
        &mut self,
        stopped: Stopped<'id>,
        data: Self::Data,
        units: UnitsHome<'id>,
    ) -> Retired<'id> {
        self.call();
        drop(data);
        // After a retirement the state is the state after the install.
        self.places.iter_mut().for_each(|place| *place = None);
        self.ring.clear();
        self.key.retire(stopped, units)
    }

    fn release(&mut self, retired: Retired<'id>, right: StopRight<'id>) -> Self::Released<'_> {
        self.call();
        self.key.release(retired, right);
    }
}

impl<'id, const SINGLES: usize, const DATA: usize, const PLACES: usize, const TX_FRONT: usize>
    DataPort<'id> for ModelPort<'id, SINGLES, DATA, PLACES, TX_FRONT>
{
    type Link = ModelLink;
    type Memory = ModelMemory;
    const TX_FRONT: usize = TX_FRONT;
    const TX_BEHIND: usize = MODEL_TX_BEHIND;
    type Ack = ();
    type BlockAck = ();
    type Denial = ();

    fn poll_facts(&mut self, _cx: &mut Context<'_>) -> Facts {
        self.call();
        let on_air = self
            .queues
            .iter()
            .any(|queue| queue.single.is_some() && (queue.air != Air::Hold || queue.withdrawn))
            || self.arenas.iter().any(|arena| {
                arena.air.is_some_and(|air| air != Air::Hold) || arena.ended.is_some()
            });
        let staging = self.anomaly != Some(Anomaly::WalkerLast);
        Facts {
            tx: on_air.then_some(TxFacts::events(1)),
            tbtt: Signal::Quiet,
            rx: if staging && !self.ring.is_empty() && self.free_places() > 0 {
                Signal::Raised
            } else {
                Signal::Quiet
            },
            needs_stop: if self.parts.has(Part::NeedsStop) {
                Signal::Raised
            } else {
                Signal::Quiet
            },
        }
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn sample_clock(&mut self) -> Result<Ieee80211Instant, ClockError> {
        self.call();
        Ok(Ieee80211Instant::from_micros(0))
    }

    fn channel(&self) -> Channel {
        self.channel.channel()
    }

    fn ampdu_capabilities(&self) -> AmpduCapabilities {
        AmpduCapabilities {
            max_subframes: u16::from(MODEL_ARENA_SLOTS),
            formats: PhyFormatSet::HT.union(PhyFormatSet::HE),
            max_length: &MODEL_AMPDU_LENGTHS,
        }
    }

    fn open(&mut self, idle: IdleArena<'id>, _link: &Self::Link, _tid: Tid) -> Arena<'id> {
        self.call();
        if let Some(arena) = self.arenas.get_mut(side_index(idle.side())) {
            *arena = ArenaBuffer::default();
        }
        self.key.open(idle)
    }

    fn place<'a>(
        &'a mut self,
        arena: &'a mut Arena<'id>,
        _length: MpduLength<Self::Caps>,
    ) -> Result<Place<'a, 'id, Self>, Complete> {
        self.call();
        if arena.frames() >= MODEL_ARENA_SLOTS {
            return Err(Complete);
        }
        let placing = self.key.placing();
        Ok(Place::new(placing, self, arena))
    }

    fn write_frame(
        &mut self,
        _token: Sealed,
        arena: &mut Arena<'id>,
        body: Self::Memory,
        header: impl FnOnce(&mut [u8; HEADROOM]) -> HeaderLen,
    ) {
        let mut slot = lay_out(self.tx, &body);
        let headroom = self.tx.headroom_at();
        // The engine writes the front into the slot's headroom itself.
        let written = match slot
            .get_mut(headroom..headroom.saturating_add(HEADROOM))
            .and_then(|room| <&mut [u8; HEADROOM]>::try_from(room).ok())
        {
            Some(room) => header(room),
            None => {
                // The slot holds its headroom by its geometry.
                self.counts.overwritten = self.counts.overwritten.saturating_add(1);
                let mut room = body.front;
                header(&mut room)
            }
        };
        let mut front = [0; HEADROOM];
        if let Some(room) = slot.get(headroom..headroom.saturating_add(HEADROOM)) {
            for (to, from) in front.iter_mut().zip(room) {
                *to = *from;
            }
        }
        if !whole(self.tx, &slot, body.len) {
            self.counts.overwritten = self.counts.overwritten.saturating_add(1);
        }
        let header = u8::try_from(written.get()).unwrap_or(u8::MAX);
        self.fronts.retain(|(id, _)| *id != body.id);
        self.fronts.push((
            body.id,
            front
                .iter()
                .skip(HEADROOM.saturating_sub(written.get()))
                .copied()
                .collect(),
        ));
        if let Some(buffer) = self.arenas.get_mut(side_index(arena.side())) {
            buffer.frames.push(Frame {
                memory: body,
                header,
                front,
                slot,
                attempts: 0,
            });
        }
    }

    fn publish(
        &mut self,
        arena: Arena<'id>,
        how: Ppdu<'id, Self::Caps>,
        _gate: &GateOpen<'id>,
        _now: Instant,
    ) -> OnAir<'id> {
        self.call();
        // A single goes alone: no second frame fits its limit.
        let limit = match how {
            Ppdu::Single(_) => 0,
            Ppdu::Aggregate { rate, .. } => MODEL_AMPDU_LENGTHS
                .at(rate.rate().get())
                .map_or(0, core::num::NonZeroU32::get),
        };
        let mut record = None;
        let air = self.air_of(oer_protocols_ieee80211_wire::qos::WmmAccessCategory::BestEffort);
        let prefix = self.prefix.take();
        let buffer = self.arenas.get_mut(side_index(arena.side()));
        let mut sent: u8 = 0;
        if let Some(buffer) = buffer {
            let mut length: u32 = 0;
            for frame in &buffer.frames {
                // A subframe: its delimiter, its MPDU, its padding to four.
                let mpdu = u32::from(frame.header).saturating_add(u32::from(frame.memory.len));
                let subframe = mpdu.saturating_add(4).saturating_add(3) & !3;
                let next = length.saturating_add(subframe);
                if sent > 0 && next > limit {
                    break;
                }
                length = next;
                sent = sent.saturating_add(1);
            }
            sent = sent
                .max(1)
                .min(u8::try_from(buffer.frames.len()).unwrap_or(u8::MAX))
                .min(prefix.unwrap_or(u8::MAX).max(1));
            let broken = buffer
                .frames
                .iter()
                .filter(|frame| !whole(self.tx, &frame.slot, frame.memory.len))
                .count();
            self.counts.overwritten = self
                .counts
                .overwritten
                .saturating_add(u32::try_from(broken).unwrap_or(u32::MAX));
            buffer.sent = sent;
            buffer.went = if sent > 1 {
                Went::Aggregate
            } else {
                Went::Single
            };
            buffer.air = Some(air);
            buffer.ended = None;
            let went_at = match (how, buffer.went) {
                (Ppdu::Single(plan) | Ppdu::Aggregate { alone: plan, .. }, Went::Single) => {
                    plan.first().rate.get()
                }
                (Ppdu::Aggregate { rate, .. }, Went::Aggregate) => rate.rate().get(),
                (Ppdu::Single(plan), Went::Aggregate) => plan.first().rate.get(),
            };
            buffer.rate = Some(went_at);
            let frames = buffer
                .frames
                .iter_mut()
                .take(usize::from(sent))
                .map(|frame| {
                    let front = frame.on_air();
                    frame.attempts = frame.attempts.saturating_add(1);
                    front
                })
                .collect();
            let protection = match (how, buffer.went) {
                (Ppdu::Aggregate { protection, .. }, Went::Aggregate) => std::vec![protection],
                (Ppdu::Single(plan) | Ppdu::Aggregate { alone: plan, .. }, _) => {
                    plan.stages().map(|stage| stage.protection).collect()
                }
            };
            record = Some(Publication {
                single: buffer.went,
                rate: went_at,
                protection,
                frames,
            });
        }
        self.published.extend(record);
        self.key.publish(arena, sent)
    }

    fn withdraw(
        &mut self,
        arena: Arena<'id>,
        home: &mut impl FnMut(Self::Memory),
    ) -> IdleArena<'id> {
        self.call();
        if let Some(buffer) = self.arenas.get_mut(side_index(arena.side())) {
            for frame in buffer.frames.drain(..) {
                home(frame.memory);
            }
            *buffer = ArenaBuffer::default();
        }
        self.key.withdraw(arena)
    }

    fn aggregate_ended(
        &mut self,
        _facts: TxFacts,
        on_air: &OnAir<'id>,
        _now: Instant,
    ) -> Option<&Ended<Self::Ack, Self::BlockAck, Self::Denial>> {
        self.call();
        let acknowledge = self.acknowledge;
        // The air answers at the end, as it is set then.
        let now_air = self.air_of(oer_protocols_ieee80211_wire::qos::WmmAccessCategory::BestEffort);
        let buffer = self.arenas.get_mut(side_index(on_air.side()))?;
        let rate = buffer.rate?;
        if buffer.ended.is_none() {
            let air = match (buffer.air, now_air) {
                (None, _) | (Some(_), Air::Hold) => return None,
                (Some(_), air) => air,
            };
            buffer.air = None;
            let (outcome, acknowledged) = match (buffer.went, air) {
                (Went::Single, Air::Timeout) => (
                    ArenaOutcome::Single(TxOutcome::Indeterminate(
                        IndeterminateCause::HardwareTimeout,
                    )),
                    FrameSet::EMPTY,
                ),
                (Went::Aggregate, Air::Timeout) => (
                    ArenaOutcome::Aggregate(TxOutcome::Indeterminate(
                        IndeterminateCause::HardwareTimeout,
                    )),
                    FrameSet::EMPTY,
                ),
                (Went::Single, Air::Ack) => (
                    ArenaOutcome::Single(TxOutcome::Ended(SingleEnd::Delivered {
                        air: Self::rate_air(rate),
                        response: SingleResponse::Ack(()),
                    })),
                    on_air.sent(),
                ),
                (Went::Single, Air::NoAck) => (
                    ArenaOutcome::Single(TxOutcome::Ended(SingleEnd::AckTimeout(Self::rate_air(
                        rate,
                    )))),
                    FrameSet::EMPTY,
                ),
                (Went::Single, Air::Deny | Air::Hold) => (
                    ArenaOutcome::Single(TxOutcome::Ended(SingleEnd::AirDenied {
                        denial: (),
                        air: None,
                    })),
                    FrameSet::EMPTY,
                ),
                (Went::Aggregate, Air::Ack) => {
                    // The BlockAck acknowledges the frames the lever names,
                    // every frame that went without it.
                    let acknowledged = FrameSet::from_bits(
                        acknowledge.map_or(on_air.sent().bits(), FrameSet::bits)
                            & on_air.sent().bits(),
                    );
                    (
                        ArenaOutcome::Aggregate(TxOutcome::Ended(AggregateEnd::Delivered {
                            air: Self::rate_air(rate),
                            report: BlockAckReport::new(SequenceNumber::ZERO, acknowledged.bits()),
                            ack: (),
                        })),
                        acknowledged,
                    )
                }
                (Went::Aggregate, Air::NoAck) => (
                    ArenaOutcome::Aggregate(TxOutcome::Ended(AggregateEnd::AckTimeout(
                        Self::rate_air(rate),
                    ))),
                    FrameSet::EMPTY,
                ),
                (Went::Aggregate, Air::Deny | Air::Hold) => (
                    ArenaOutcome::Aggregate(TxOutcome::Ended(AggregateEnd::AirDenied {
                        denial: (),
                        air: None,
                    })),
                    FrameSet::EMPTY,
                ),
            };
            buffer.ended = Some(Ended {
                outcome,
                sent: on_air.sent(),
                acknowledged,
            });
            self.acknowledge = None;
        }
        buffer.ended.as_ref()
    }

    fn settle(
        &mut self,
        on_air: OnAir<'id>,
        keep: FrameSet,
        home: &mut impl FnMut(Self::Memory),
    ) -> Settled<'id> {
        self.call();
        let sent = on_air.sent();
        let mut kept: u8 = 0;
        if let Some(buffer) = self.arenas.get_mut(side_index(on_air.side())) {
            let mut stay = Vec::new();
            for (position, frame) in buffer.frames.drain(..).enumerate() {
                let position = u8::try_from(position).unwrap_or(u8::MAX);
                // A frame that did not go stays, with no attempt spent.
                if !sent.contains(position) || keep.contains(position) {
                    stay.push(frame);
                } else {
                    home(frame.memory);
                }
            }
            kept = u8::try_from(stay.len()).unwrap_or(u8::MAX);
            *buffer = ArenaBuffer {
                frames: stay,
                ..ArenaBuffer::default()
            };
        }
        self.key.settle(on_air, kept)
    }

    fn serve(
        &mut self,
        _facts: TxFacts,
        // The model has no station TBTT schedule.
        _tbtt: Signal,
        _now: Instant,
        sink: &mut impl PortSink<'id, Self::Ack, Self::Denial>,
    ) {
        self.call();
        for (index, category) in oer_protocols_ieee80211_contracts_port::QUEUES
            .iter()
            .enumerate()
        {
            let Some(queue) = self.queues.get_mut(index) else {
                continue;
            };
            let Some(place) = queue.single else {
                continue;
            };
            let rate = queue.rate.unwrap_or(PhyRate::Legacy(LegacyRate::Ofdm6M));
            let outcome: SingleOutcome<(), ()> = match queue.air {
                _ if queue.withdrawn => TxOutcome::Withdrawn(WithdrawCause::Disabled),
                Air::Hold => continue,
                Air::Ack => TxOutcome::Ended(SingleEnd::Delivered {
                    air: Self::rate_air(rate),
                    response: SingleResponse::Ack(()),
                }),
                Air::NoAck => TxOutcome::Ended(SingleEnd::AckTimeout(Self::rate_air(rate))),
                Air::Timeout => TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout),
                Air::Deny => TxOutcome::Ended(SingleEnd::AirDenied {
                    denial: (),
                    air: None,
                }),
            };
            queue.single = None;
            queue.withdrawn = false;
            let (place, right) = self.key.single_end(place, *category);
            sink.single_ended(outcome, place, right);
        }
    }

    fn write_single(
        &mut self,
        place: &mut SinglePlace<'id>,
        fill: impl FnOnce(&mut [u8]) -> MpduLength<Self::Caps>,
    ) {
        self.call();
        let mut buffer = std::vec![0; MODEL_MAX_MPDU];
        let length = fill(&mut buffer).get();
        buffer.truncate(length);
        if let Some(slot) = self.singles.get_mut(usize::from(place.index())) {
            *slot = buffer;
        }
    }

    fn publish_single(
        &mut self,
        place: SinglePlace<'id>,
        queue: QueueRight<'id>,
        _gate: &GateOpen<'id>,
        frame: &TxFrame<'id, Self::Caps>,
        _now: Instant,
    ) {
        self.call();
        let rate = match frame {
            TxFrame::Individual { plan, .. } => Some(plan.first().rate.get()),
            TxFrame::IndividualNoAck { rate, .. } => Some(rate.rate().get()),
            TxFrame::Group { rate, .. } => Some(rate.get()),
            TxFrame::BlockAckRequest { plan, .. } => Some(plan.first().rate.rate().get()),
        };
        if let Some(slot) = self.queues.get_mut(queue_index(queue.category())) {
            slot.single = Some(place.index());
            slot.rate = rate;
        }
        self.key.single_published(place, queue);
    }

    fn begin_close(
        &mut self,
        place: CommandPlace<'id>,
        command: CloseGate<'id>,
        _now: Instant,
    ) -> InCommand<'id, GateOpen<'id>> {
        let CloseGate(open) = command;
        self.begin(place, open)
    }

    fn poll_close(
        &mut self,
        command: InCommand<'id, GateOpen<'id>>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Ack, Self::Denial>,
    ) -> CommandPoll<'id, GateOpen<'id>, GateClosed<'id>> {
        self.polled(now, sink);
        if self.queues.iter().any(|queue| queue.single.is_some())
            || self.arenas.iter().any(|arena| arena.air.is_some())
        {
            // The barrier waits for what is on the air.
            return CommandPoll::Pending(command);
        }
        let (place, open) = self.ended(command);
        self.parts.off(Part::Gate);
        self.program(Programmed::Gate(false));
        CommandPoll::Ended(place, self.issuer.close(open))
    }

    fn begin_pairwise(
        &mut self,
        place: CommandPlace<'id>,
        command: PairwiseCommand<'id>,
        _now: Instant,
    ) -> InCommand<'id, PairwiseHeld<'id>> {
        // The model programs none of the key's values: they end here.
        let held = match command {
            PairwiseCommand::Install { slot, .. } => PairwiseHeld::Installing(slot),
            PairwiseCommand::Remove(installed) => PairwiseHeld::Removing(installed),
        };
        self.begin(place, held)
    }

    fn poll_pairwise(
        &mut self,
        command: InCommand<'id, PairwiseHeld<'id>>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Ack, Self::Denial>,
    ) -> CommandPoll<'id, PairwiseHeld<'id>, PairwiseEnd<'id>> {
        self.polled(now, sink);
        // A key's command waits, as the gate's closing does, for the arena
        // on the air: what it misses would be retried under another key.
        if self.arenas.iter().any(|arena| arena.air.is_some()) {
            return CommandPoll::Pending(command);
        }
        let expire = self.parts.take(Part::Expire);
        let (place, running) = self.ended(command);
        let end = match running {
            PairwiseHeld::Installing(slot) => {
                if expire {
                    self.parts.off(Part::Pairwise);
                    self.program(Programmed::Pairwise(false));
                    PairwiseEnd::Empty(slot)
                } else {
                    self.parts.on(Part::Pairwise);
                    self.program(Programmed::Pairwise(true));
                    PairwiseEnd::Installed(self.key.pairwise_installed(slot))
                }
            }
            PairwiseHeld::Removing(key) => {
                self.parts.off(Part::Pairwise);
                self.program(Programmed::Pairwise(false));
                PairwiseEnd::Empty(self.key.pairwise_removed(key))
            }
        };
        CommandPoll::Ended(place, end)
    }

    fn begin_group(
        &mut self,
        place: CommandPlace<'id>,
        command: GroupCommand<'id>,
        _now: Instant,
    ) -> InCommand<'id, GroupHeld<'id>> {
        let held = match command {
            GroupCommand::Install { slot, .. } => GroupHeld::Installing(slot),
            GroupCommand::Replace { installed, .. } => GroupHeld::Replacing(installed),
            GroupCommand::Select(installed) => GroupHeld::Selecting(installed),
            GroupCommand::Remove(installed) => GroupHeld::Removing(installed),
        };
        self.begin(place, held)
    }

    fn poll_group(
        &mut self,
        command: InCommand<'id, GroupHeld<'id>>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Ack, Self::Denial>,
    ) -> CommandPoll<'id, GroupHeld<'id>, GroupEnd<'id>> {
        self.polled(now, sink);
        let expire = self.parts.take(Part::Expire);
        let (place, running) = self.ended(command);
        let end = match running {
            GroupHeld::Installing(slot) => {
                let index = slot.index();
                if expire {
                    self.set_group(index, Slot::Empty);
                    GroupEnd::Empty(slot)
                } else {
                    self.set_group(index, Slot::Held);
                    GroupEnd::Installed(self.key.group_installed(slot))
                }
            }
            GroupHeld::Replacing(installed) | GroupHeld::Selecting(installed) => {
                let index = installed.index();
                if expire {
                    self.set_group(index, Slot::Empty);
                    GroupEnd::Empty(self.key.group_removed(installed))
                } else {
                    self.set_group(index, Slot::Held);
                    GroupEnd::Installed(installed)
                }
            }
            GroupHeld::Removing(installed) => {
                let index = installed.index();
                self.set_group(index, Slot::Empty);
                GroupEnd::Empty(self.key.group_removed(installed))
            }
        };
        CommandPoll::Ended(place, end)
    }

    fn begin_agreement(
        &mut self,
        place: CommandPlace<'id>,
        command: AgreementCommand<'id, Self::Caps>,
        _now: Instant,
    ) -> InCommand<'id, AgreementHeld<'id>> {
        let held = match command {
            AgreementCommand::Add { slot, .. } => AgreementHeld::Adding(slot),
            AgreementCommand::End(agreement) => AgreementHeld::Ending(agreement),
        };
        self.begin(place, held)
    }

    fn poll_agreement(
        &mut self,
        command: InCommand<'id, AgreementHeld<'id>>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Ack, Self::Denial>,
    ) -> CommandPoll<'id, AgreementHeld<'id>, AgreementEnd<'id>> {
        self.polled(now, sink);
        let expire = self.parts.take(Part::Expire);
        let (place, running) = self.ended(command);
        let end = match running {
            AgreementHeld::Adding(slot) => {
                let index = slot.index();
                if expire {
                    AgreementEnd::Empty(slot)
                } else {
                    self.set_agreement(index, Slot::Held);
                    AgreementEnd::Added(self.key.agreement_added(slot))
                }
            }
            AgreementHeld::Ending(agreement) => {
                let index = agreement.index();
                self.set_agreement(index, Slot::Empty);
                AgreementEnd::Empty(self.key.agreement_ended(agreement))
            }
        };
        CommandPoll::Ended(place, end)
    }

    fn begin_value(
        &mut self,
        place: CommandPlace<'id>,
        command: ValueCommand<Self::Caps>,
        _now: Instant,
    ) -> InCommand<'id, ValueKind> {
        let kind = match command {
            ValueCommand::Configure(_) => ValueKind::Configure,
            ValueCommand::SetEdca(_) => ValueKind::SetEdca,
            ValueCommand::SetBssColor(_) => ValueKind::SetBssColor,
            ValueCommand::SetHe(_) => ValueKind::SetHe,
        };
        self.begin(place, kind)
    }

    fn poll_value(
        &mut self,
        command: InCommand<'id, ValueKind>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Ack, Self::Denial>,
    ) -> CommandPoll<'id, ValueKind, ValueEnd> {
        self.polled(now, sink);
        let end = if self.parts.take(Part::Expire) {
            ValueEnd::Expired
        } else {
            ValueEnd::Done
        };
        let (place, running) = self.ended(command);
        if end == ValueEnd::Done {
            self.program(match running {
                ValueKind::Configure => Programmed::Configure,
                ValueKind::SetEdca => Programmed::Edca,
                ValueKind::SetBssColor => Programmed::Color,
                ValueKind::SetHe => Programmed::He,
            });
        }
        CommandPoll::Ended(place, end)
    }

    fn open_gate(&mut self, gate: GateClosed<'id>) -> GateOpen<'id> {
        self.call();
        self.parts.on(Part::Gate);
        self.program(Programmed::Gate(true));
        self.issuer.open(gate)
    }
}

impl<'id, const SINGLES: usize, const DATA: usize, const PLACES: usize, const TX_FRONT: usize>
    ModelPort<'id, SINGLES, DATA, PLACES, TX_FRONT>
{
    fn set_group(&mut self, index: u8, slot: Slot) {
        let installed = matches!(slot, Slot::Held);
        if let Some(place) = self.groups.get_mut(usize::from(index)) {
            *place = installed;
        }
        self.program(Programmed::Group(index, installed));
    }

    fn set_agreement(&mut self, index: u8, slot: Slot) {
        let running = matches!(slot, Slot::Held);
        if let Some(place) = self.agreements.get_mut(usize::from(index)) {
            *place = running;
        }
        self.program(Programmed::Agreement(index, running));
    }

    /// A command began on the one place.
    fn begin<H>(&mut self, place: CommandPlace<'id>, held: H) -> InCommand<'id, H> {
        self.call();
        self.commanding = Commanding::Running;
        self.key.began(place, held)
    }

    /// A command's poll: the singles' ends first, when the lever says so.
    fn polled(&mut self, now: Instant, sink: &mut impl PortSink<'id, (), ()>) {
        self.call();
        if core::mem::take(&mut self.serve_in_poll) {
            self.serve(TxFacts::DUE, Signal::Quiet, now, sink);
        }
    }

    /// A command ended: its place back with what it held.
    fn ended<H>(&mut self, command: InCommand<'id, H>) -> (CommandPlace<'id>, H) {
        self.commanding = Commanding::Idle;
        self.key.command_ended(command)
    }
}

impl<'id, const SINGLES: usize, const DATA: usize, const PLACES: usize, const TX_FRONT: usize>
    ReceivePort<'id> for ModelPort<'id, SINGLES, DATA, PLACES, TX_FRONT>
{
    type Meta = RxMeta;
    type Lent = ModelLent;
    const STAGING: usize = PLACES;

    fn round_open(&mut self, _seal: &Sealed, _now: Instant) -> usize {
        self.call();
        self.counts.rounds = self.counts.rounds.saturating_add(1);
        let returned = places_of(self.returns.replace(0));
        for &place in &returned {
            if let Some(slot) = self.places.get_mut(usize::from(place)) {
                *slot = None;
            }
        }
        returned.len()
    }

    fn round_service(&mut self, _seal: &Sealed) -> StagedPlaces {
        self.counts.services = self.counts.services.saturating_add(1);
        let mut staged = StagedPlaces::EMPTY;
        if self.anomaly == Some(Anomaly::WalkerLast) {
            // The place that saw the anomaly gives nothing until the stop.
            return staged;
        }
        while let Some(free) = self.places.iter().position(Option::is_none) {
            if staged.places().len() >= self.frontier {
                break;
            }
            let Some((bytes, meta)) = self.ring.pop_front() else {
                break;
            };
            self.counts.decodes = self.counts.decodes.saturating_add(1);
            let Some(head) = RxHead::of(&bytes) else {
                self.counts.dropped = self.counts.dropped.saturating_add(1);
                continue;
            };
            let Ok(place) = u8::try_from(free) else {
                break;
            };
            if let Some(slot) = self.places.get_mut(free) {
                *slot = Some(Unit {
                    staged: Staged {
                        head,
                        meta,
                        lendable: Some(Lendable::staged(_seal)),
                    },
                    bytes,
                    lent: false,
                });
            }
            if !staged.push(place) {
                break;
            }
        }
        staged
    }

    fn round_unit(&self, _seal: &Sealed, place: u8) -> Option<(&Staged<Self::Meta>, &[u8])> {
        let unit = self.places.get(usize::from(place))?.as_ref()?;
        Some((&unit.staged, unit.bytes.as_slice()))
    }

    fn round_lend(&mut self, _seal: &Sealed, place: u8) -> Self::Lent {
        if let Some(Some(unit)) = self.places.get_mut(usize::from(place)) {
            unit.lent = true;
        }
        ModelLent {
            place,
            returns: Rc::clone(&self.returns),
        }
    }

    fn round_write_addresses(
        &mut self,
        _seal: &Sealed,
        place: u8,
        addresses: EthernetAddresses,
    ) -> Result<(), OutsideTheUnit> {
        let Some(Some(unit)) = self.places.get_mut(usize::from(place)) else {
            return Err(OutsideTheUnit);
        };
        let end = usize::from(addresses.ether_type);
        let Some(target) = end
            .checked_sub(12)
            .and_then(|start| unit.bytes.get_mut(start..end))
            .and_then(|target| target.first_chunk_mut::<12>())
        else {
            return Err(OutsideTheUnit);
        };
        let [d0, d1, d2, d3, d4, d5] = addresses.destination;
        let [s0, s1, s2, s3, s4, s5] = addresses.source;
        *target = [d0, d1, d2, d3, d4, d5, s0, s1, s2, s3, s4, s5];
        Ok(())
    }

    fn round_close(&mut self, _seal: &Sealed, back: u64) -> Rest {
        self.call();
        self.counts.closes = self.counts.closes.saturating_add(1);
        for place in 0..PLACES {
            if back & (1_u64 << place) != 0
                && let Some(slot) = self.places.get_mut(place)
            {
                *slot = None;
            }
        }
        if !self.ring.is_empty() && self.free_places() > 0 {
            Rest::Round
        } else {
            Rest::Signal
        }
    }
}

#[cfg(test)]
mod tx_slot {
    use super::*;

    type Port = ModelPort<'static, 0, 0>;
    const SLOT: TxSlot<Port> = TxSlotOf::<'static, Port, MODEL_TX_BODY, MODEL_TX_SLOT>::SLOT;

    fn memory(len: u16) -> ModelMemory {
        ModelMemory {
            id: 7,
            len,
            front: [0x11; HEADROOM],
        }
    }

    /// The model lays its own octets out in front of the headroom and
    /// behind the body and its MIC's room, and finds them whole.
    #[test]
    fn a_slot_is_laid_out_around_the_body_and_found_whole() {
        let body = 100;
        let slot = lay_out(SLOT, &memory(body));
        assert_eq!(slot.len(), MODEL_TX_SLOT);
        assert!(
            slot[..MODEL_TX_FRONT]
                .iter()
                .all(|octet| *octet == FRONT_OCTET)
        );
        assert_eq!(
            &slot[MODEL_TX_FRONT..MODEL_TX_FRONT + HEADROOM],
            &[0x11; HEADROOM]
        );
        let body_at = MODEL_TX_FRONT + HEADROOM;
        assert_eq!(body_at % 4, 0, "the body lies on a word");
        assert!(slot[body_at..body_at + 100].iter().all(|octet| *octet == 7));
        let behind = body_at + 100 + oer_protocols_ieee80211_contracts_port::MIC_ROOM;
        assert_eq!(SLOT.behind(100), Some(behind..behind + MODEL_TX_BEHIND));
        assert!(
            slot[behind - 8..behind].iter().all(|octet| *octet == 0),
            "the MIC's room"
        );
        assert!(
            slot[behind..behind + MODEL_TX_BEHIND]
                .iter()
                .all(|octet| *octet == BEHIND_OCTET)
        );
        assert!(whole(SLOT, &slot, body));
    }

    /// An octet written in front of the headroom, or behind the body and
    /// its MIC, is found; one in the MIC's room is not the model's.
    #[test]
    fn an_octet_written_outside_the_headroom_and_the_body_is_found() {
        let body = 100;
        let behind = SLOT.behind(100).unwrap().start;
        for at in [0, MODEL_TX_FRONT - 1, behind, behind + MODEL_TX_BEHIND - 1] {
            let mut slot = lay_out(SLOT, &memory(body));
            slot[at] ^= 1;
            assert!(!whole(SLOT, &slot, body), "an octet at {at}");
        }
        let mut slot = lay_out(SLOT, &memory(body));
        slot[behind - 1] ^= 1;
        assert!(
            whole(SLOT, &slot, body),
            "the MIC's room is the port's to fill"
        );
    }

    /// A body longer than the slot's runs into the model's octets behind.
    #[test]
    fn a_body_longer_than_the_slots_is_found() {
        let body = MODEL_TX_BODY as u16 + 1;
        let slot = lay_out(SLOT, &memory(body));
        assert!(!whole(SLOT, &slot, body));
    }

    /// The slot refuses at run time what it refuses at build.
    #[test]
    fn the_constructor_refuses_at_run_time_what_the_build_refuses() {
        use oer_protocols_ieee80211_contracts_port::{TxSlotRefused, tx_slot};
        let needed = tx_slot::<Port>(MODEL_TX_BODY).unwrap();
        assert_eq!(
            needed,
            MODEL_TX_FRONT + HEADROOM + MODEL_TX_BODY + 8 + MODEL_TX_BEHIND
        );
        assert!(TxSlot::<Port>::new(MODEL_TX_BODY, needed).is_ok());
        assert_eq!(
            TxSlot::<Port>::new(MODEL_TX_BODY, needed - 1).unwrap_err(),
            TxSlotRefused::Short
        );
        // The longest front and the body against the MPDU the model sends
        // without its MIC and FCS: 42 + 2 262 = 2 304.
        assert!(TxSlot::<Port>::new(2_262, 4_096).is_ok());
        assert_eq!(
            TxSlot::<Port>::new(2_263, 4_096).unwrap_err(),
            TxSlotRefused::MpduTooLong
        );
        assert_eq!(
            TxSlot::<ModelPort<'static, 0, 0, MODEL_PLACES, 14>>::new(100, 4_096).unwrap_err(),
            TxSlotRefused::BodyOffWord
        );
        assert_eq!(
            TxSlot::<Port>::new(usize::MAX, usize::MAX).unwrap_err(),
            TxSlotRefused::Overflow
        );
    }
}
