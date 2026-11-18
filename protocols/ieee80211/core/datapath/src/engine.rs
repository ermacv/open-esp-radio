//! The engine's transmit half (`design/engine-contract-2026-11-15.md`,
//! B3e, under `design/port-contract-2026-11-16.md` where the two differ):
//! one sans-IO core an interface, between the port, the network's
//! transport and the interface's controller.
//!
//! **A state is a sum.** The engine is built where it lives
//! ([`Engine::new`] is `const`): the data's state is stopped, or flowing
//! or stopping with everything the running data holds (the plane built
//! with its arenas or the access point's lanes, the gate with the
//! command's place, the rights); a peer's entry is free, or linked with
//! its link. Where a large value is built into a variant (the plane at the
//! data's start, a link at an association, the running data at its stop),
//! the copy is marked at its site for the audit of copies and frames.
//!
//! A controller lent the engine in its step reads a **view of a place** and
//! acts on its variant: a transition is a method of the variant's handle,
//! so it exists only where it may happen, and nothing outlives the step:
//!
//! - [`DataPlace`]: [`StoppedData::flows`]; [`FlowingData::peer`],
//!   [`FlowingData::gate`], [`FlowingData::stop`]; [`StoppingData::poll`];
//! - [`PeerPlace`]: `FreePeer::link`; [`LinkedPeer::unlink`],
//!   [`LinkedPeer::hold`], [`LinkedPeer::release`],
//!   [`LinkedPeer::own_number`], [`LinkedPeer::naming`], and on the
//!   station's form `LinkedPeer::tx_agreement_ended`;
//! - [`NamePlace`]: [`UnnamedTids::name`]; [`DrainingTid::release`];
//!   [`NamedTid::release`], and on the station's form
//!   `NamedTid::tx_agreement`: a TID's agreement begins only on its named
//!   number;
//! - [`GatePlace`]: [`ShutGate::open`]; [`OpenGate::close`]; the closing
//!   ends in the port, and the closed gate goes to the controller
//!   ([`Told::gate_closed`]).
//!
//! The pump calls the sections, functions of what woke it, each lent the
//! port, the network's end, the rate policy, the events' sink and the
//! controller's end ([`Turn`]): [`Engine::offered`] (the intake: drain,
//! fill, publish, again while it moved something, within one budget; a
//! budget that ran out with more to take owes a round), [`Engine::round`]
//! and [`Engine::transmitted`] (the arena's end, the other arena at once
//! when the end allows it, the settlement, the singles' ends, the
//! command's poll, the intake). The pump makes the installation machine's
//! `Rest` of the answer.
//!
//! An interface's data goes in one of two forms ([`Form`], D1 of the
//! engine's contract): a station's in the port's two arenas ([`Arenas`],
//! the [`DataPlane`]), an access point's as single MPDUs ([`Singles`])
//! until its aggregation. A single is written into its queue's data place
//! by copy (`DataPort::write_single`): the MAC's DMA does not read the
//! network's memory on the ESP32-S31, so the memory goes home at the write.
//! An access point's data has one place a transmit queue
//! (`DataRights::data`, [`DATA_SINGLES`]); a station's layout has none.

use core::marker::PhantomData;

use oer_portable_events::{EventSink, report};
use oer_portable_time::{Duration, Instant};
pub use oer_protocols_ieee80211_contracts_port::DATA_SINGLES;
use oer_protocols_ieee80211_contracts_port::{
    AckHeader, Agreement, AgreementCommand, AgreementEnd, AgreementHeld, AgreementSlot, CloseGate,
    CommandPlace, CommandPoll, DataRights, GateClosed, GateOpen, GroupSlot, HeaderSpec, IdleArena,
    Ieee80211Capabilities, InCommand, InstalledPairwise, MpduLength, PLACES, PairwiseCommand,
    PairwiseEnd, PairwiseHeld, PairwiseSlot, PortRate, PortSink, QueueRight, ReceivePort, Received,
    RetryPlan, RxHeader, RxMetadata, Signal, SingleEnd, SingleOutcome, SinglePlace, SizedFrame,
    TbttEvent, TxFacts, TxFrame, TxOutcome, UnitHandle,
};
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_core_installation::{DataStop, Rest};
use oer_protocols_ieee80211_core_upper_mac::block_ack::{
    BankCommand, BankTicket, RX_BLOCK_ACK_WINDOW, ReplyPlace, RxAgreements, RxBlockAckStorage,
    RxBlockAckTypes,
};
use oer_protocols_ieee80211_wire::address::{Bssid, IndividualAddress};
use oer_protocols_ieee80211_wire::block_ack::frame::ADDBA_ACTION_BODY_LEN;
use oer_protocols_ieee80211_wire::data::{
    DataInterfaceRole, LLC_SNAP_HEADER_LEN, MsduHeader, PeerQos, plan_data_encapsulation,
};
use oer_protocols_ieee80211_wire::frame::{MacHeader, QosAckPolicy};
use oer_protocols_ieee80211_wire::qos::{WmmAccessCategory, WmmUserPriority};
use oer_protocols_ieee80211_wire::security::{LinkProtection, Spp};
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

use crate::cost::single_cost;
use crate::events::{DataEvent, DataLoss};
use crate::numbers::{LinkSpace, TxKey, TxNumbers};
use crate::plane::{
    DataLink, DataPlane, Intake, IntakeRefusal, OutboundMsdu, PpduPlan, RateLesson, TxWindow, Waits,
};
use crate::queues::{QueueId, TxQueues};

/// The FCS every MPDU carries on the air (IEEE Std 802.11-2020, 9.2.4.8).
const FCS_LEN: u32 = 4;

/// The network's end of the data path, which the binding implements over
/// the network's own interface. The network names no IEEE 802.11 type: the
/// engine reads an MSDU's Ethernet frame in its memory, and hands it the
/// Ethernet frames of received units. `M` is the network's handle of an
/// MSDU's memory, `L` a received unit the port lends.
pub trait Network<M, L> {
    /// The next MSDU the network offers; `None` while it offers none.
    fn take(&mut self) -> Option<M>;

    /// The length of `memory`'s Ethernet frame.
    fn length(&self, memory: &M) -> usize;

    /// The user priority the network gave `memory`'s MSDU.
    fn priority(&self, memory: &M) -> WmmUserPriority;

    /// The Ethernet frame of `memory`, `length` octets: its header, then its
    /// payload. Read for a copy.
    fn frame<'m>(&'m self, memory: &'m M, length: usize) -> &'m [u8];

    /// The memory comes home: the MSDU was delivered, lost, refused or
    /// copied out.
    fn home(&mut self, memory: M);

    /// What the network takes of received frames now. The value is read
    /// once a round and spent a delivery at a time; while the network holds
    /// a lent unit, the binding counts it against its lend limit, which
    /// leaves the ring its reserve.
    fn room(&self) -> Room;

    /// A received unit lent in place: its one Ethernet frame is `frame`.
    /// It comes home when the network drops it.
    fn lent(&mut self, permit: LendPermit, unit: L, frame: EthernetSpan);

    /// A received MSDU's Ethernet frame, to copy into the network's
    /// storage now: its unit goes back to the ring after the call.
    fn copied(&mut self, permit: CopyPermit, frame: &[u8]);
}

/// The rate policy the engine asks a publication's rates of and teaches
/// each end. A generic parameter of its sections, so its set is closed.
pub trait Rates<'id, K> {
    /// The PPDU an arena's plan goes as; `None` when the policy gives none
    /// now, and the arena waits.
    fn arena(
        &mut self,
        peer: u8,
        plan: PpduPlan,
    ) -> Option<oer_protocols_ieee80211_contracts_port::Ppdu<'id, K>>;

    /// The retry plan of a single to `peer` of `tid`; `None` when the
    /// policy gives none now, and the MSDU waits.
    fn single(&mut self, peer: u8, tid: WmmUserPriority) -> Option<RetryPlan<PortRate<'id, K>>>;

    /// What an end of `peer`'s data teaches.
    fn learn<A: Copy, B: Copy, D: Copy>(
        &mut self,
        peer: u8,
        lesson: RateLesson<A, B, D>,
        now: Instant,
    );
}

/// What the engine tells the interface's controller in a section, handed
/// over at once: the pump's end of the controller.
pub trait Told<'id> {
    /// The port served a TBTT of the station's schedule.
    fn tbtt(&mut self, event: TbttEvent);

    /// The gate's closing ended: the closed gate is the controller's again.
    fn gate_closed(&mut self, gate: GateClosed<'id>);

    /// A key's command ended.
    fn key_ended(&mut self, end: KeyEnd);

    /// The receive agreements owe a reply (an ADDBA Response or a DELBA):
    /// the station's linked peer's `rx_reply` holds it.
    fn reply_owed(&mut self);

    /// A receive agreement's command ended: the command place is free, and
    /// a closing of the gate or a key's change may begin.
    fn agreement_ended(&mut self);

    /// A received management frame or EAPOL of `peer` (`None`: of no linked
    /// peer), read in place inside the round: it goes back to the ring
    /// after the call.
    fn frame<M: RxMetadata>(&mut self, peer: Option<u8>, frame: &Received<'_, M>);

    /// The TID `peer`'s link named ([`UnnamedTids::name`]) has nothing in
    /// the port's places: `number` is the one its next data MPDU carries,
    /// for an ADDBA Request's Starting Sequence Number (IEEE Std
    /// 802.11-2020, 9.4.1.13).
    fn tid_named<const PEERS: usize>(
        &mut self,
        peer: Peer<PEERS>,
        tid: WmmUserPriority,
        number: SequenceNumber,
    );
}

/// What one section is lent: the port, the network's end, the rate policy,
/// the events' sink, the controller's end and the section's one instant.
pub struct Turn<'t, P, N, R, E, C> {
    pub port: &'t mut P,
    pub network: &'t mut N,
    pub rates: &'t mut R,
    pub events: &'t mut E,
    pub told: &'t mut C,
    pub now: Instant,
}

/// A peer of an engine of `PEERS`: an index of its table, checked when it
/// is made.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Peer<const PEERS: usize>(u8);

impl<const PEERS: usize> Peer<PEERS> {
    /// The peer at `index`, when the table has it.
    pub const fn new(index: u8) -> Option<Self> {
        if (index as usize) < PEERS {
            Some(Self(index))
        } else {
            None
        }
    }

    /// Its index.
    pub const fn index(self) -> u8 {
        self.0
    }
}

/// A station's one peer: its access point.
pub const ACCESS_POINT: Peer<1> = match Peer::new(0) {
    Some(peer) => peer,
    None => panic!("a station's table has its access point"),
};

/// The link's numbers have no space of that kind: a link without QoS has
/// no TID spaces (its peer did not negotiate QoS).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NoSpace;

mod sealed {
    pub trait Sealed {}
}

/// How an interface's data goes: a closed set.
pub trait Form: sealed::Sealed {
    /// What the form holds while its data runs.
    type Running<'id, K>;
    /// What a linked peer's entry keeps of its link.
    type Entry<L, T>;
    /// The data's rights of the form's layout, which the installation
    /// machine hands over when the data flows and takes back when it
    /// stops.
    type Rights<'id, const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize>;
}

/// A station's data, in the port's two arenas.
#[derive(Debug)]
pub struct Arenas;

/// An access point's data, as single MPDUs written by copy.
#[derive(Debug)]
pub struct Singles;

impl sealed::Sealed for Arenas {}
impl sealed::Sealed for Singles {}

impl Form for Arenas {
    type Running<'id, K> = StationRunning<'id, K>;
    type Entry<L, T> = StationEntry<L>;
    type Rights<'id, const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize> =
        DataRights<'id, SINGLES, GROUPS, AGREEMENTS, 0>;
}

impl Form for Singles {
    type Running<'id, K> = ApRunning<'id, K>;
    type Entry<L, T> = ClientLink;
    type Rights<'id, const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize> =
        DataRights<'id, SINGLES, GROUPS, AGREEMENTS, DATA_SINGLES>;
}

/// A station's link to its access point, as its controller links it: the
/// plane's data state, the port's link its arenas open for, and the link's
/// transmit numbers.
pub struct StationLink<L> {
    pub data: DataLink,
    pub port: L,
    pub numbers: TxNumbers,
}

/// What a station's linked entry keeps; the plane holds the link's data
/// state.
pub struct StationEntry<L> {
    port: L,
    numbers: TxNumbers,
}

/// An access point's link to one client: its address, its own, whether it
/// negotiated QoS and the link's transmit numbers. Its data is open:
/// protection comes with the access point's keys.
pub struct ClientLink {
    pub client: [u8; 6],
    pub access_point: [u8; 6],
    pub qos: PeerQos,
    pub numbers: TxNumbers,
}

/// What a station's data holds while it runs: the plane, built with the
/// two arenas, and the transmit queues' rights, which its data does not
/// lend.
pub struct StationRunning<'id, K> {
    plane: DataPlane<'id, K>,
    /// In [`QUEUES`](oer_protocols_ieee80211_contracts_port::QUEUES)'
    /// order.
    queues: [QueueRight<'id>; 4],
}

/// What an access point's data holds while it runs: a lane a transmit
/// queue, in [`WmmAccessCategory::select`]'s order, and the arenas, which
/// its data does not use until its aggregation.
pub struct ApRunning<'id, K> {
    lanes: [Lane<'id, K>; 4],
    arenas: [IdleArena<'id>; 2],
}

/// A transmit queue of an access point's data.
enum Lane<'id, K> {
    /// Free: its queue's right and its data place.
    Free {
        right: QueueRight<'id>,
        place: SinglePlace<'id>,
    },
    /// A single of the data on it: the port holds the queue's right and the
    /// place until its end.
    Air(InFlight<'id, K>),
}

/// What the engine keeps of a single in flight, for its end.
struct InFlight<'id, K> {
    peer: u8,
    queue: QueueId,
    plan: RetryPlan<PortRate<'id, K>>,
    psdu: u32,
}

/// The gate the controller lent.
enum Gate<'id> {
    /// The controller holds it, closed.
    Shut,
    /// Open: arenas and singles go.
    Open(GateOpen<'id>),
}

/// The station's pairwise key: its slot empty, or the port's key installed
/// with the link's transmit key of packet numbers `T`, which exists exactly
/// while the port's does.
enum StationKey<'id, T> {
    Empty(PairwiseSlot<'id>),
    Installed(InstalledPairwise<'id>, T),
}

/// A key the controller installs: the port's entry (its peer, the key and
/// the association's SPP A-MSDU decision) and the link's transmit key of
/// packet numbers.
pub struct KeyChange<T> {
    pub peer: IndividualAddress,
    pub key: Ccmp128Key,
    pub spp: Spp,
    pub transmit: T,
}

/// What the station's key slot is to become.
pub enum KeyTarget<T> {
    /// Empty: the data's stop.
    Empty,
    /// This key installed: a first installation, or a rekey.
    Install(KeyChange<T>),
}

/// Where a key's command is.
enum KeyStep<T> {
    /// The installed key goes, and its transmit key `old` with it; then
    /// the slot becomes `then`.
    Removing { old: T, then: KeyTarget<T> },
    /// A key is installed; `transmit` is the link's once it is.
    Installing { transmit: T },
}

/// What the interface's command place is doing, each variant holding what
/// exists beside it. A key's command and the gate's closing stop the
/// intake; no other command runs in this half yet.
enum Interface<'id, T> {
    /// No command runs.
    Idle {
        gate: Gate<'id>,
        place: CommandPlace<'id>,
        key: StationKey<'id, T>,
    },
    /// A key's command runs: the key's slot is the port's.
    Keying {
        gate: Gate<'id>,
        command: InCommand<'id, PairwiseHeld<'id>>,
        step: KeyStep<T>,
    },
    /// A receive agreement's command runs (its addition or its end, for
    /// the reorder's bank `bank`); the data goes on.
    Agreeing {
        gate: Gate<'id>,
        command: InCommand<'id, AgreementHeld<'id>>,
        ticket: BankTicket,
        key: StationKey<'id, T>,
    },
    /// The gate's closing runs: the gate is the port's.
    Closing {
        command: InCommand<'id, GateOpen<'id>>,
        key: StationKey<'id, T>,
    },
    /// Inside one method, between two of the above: the port's command
    /// takes the place and its right by value, and no variant is without
    /// them, so safe Rust moves them out through a value left in their
    /// place. Never seen outside it.
    InTransition,
}

/// The owners the receive agreements hold: the port's slots and running
/// agreements, and the handles of the units the reorder retains.
pub struct EngineAgreements<'id, K>(PhantomData<(Brand<'id>, K)>);

/// The brand of one installation, invariant in `'id`.
type Brand<'id> = fn(&'id ()) -> &'id ();

impl<'id, K: Ieee80211Capabilities> RxBlockAckTypes for EngineAgreements<'id, K> {
    type Slot = AgreementSlot<'id>;
    type Running = Agreement<'id>;
    type Frame = UnitHandle<'id>;
    type Caps = K;
}

/// The receive agreements of the data: `AGREEMENTS` banks, each the port's
/// slot, and the reorder's frames (its window; the engine's count keeps a
/// place free for the frame that closes a hole).
type Agreements<'id, K, const AGREEMENTS: usize, const WINDOW: usize> =
    RxBlockAckStorage<EngineAgreements<'id, K>, u8, AGREEMENTS, WINDOW, WINDOW>;

/// The stopping data's agreement slots, taken in place when every bank is
/// free; `None` otherwise, and nothing moved.
fn agreement_slots<
    'id,
    F: Form,
    K: Ieee80211Capabilities,
    T,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
>(
    data: &mut Data<'id, F, K, T, SINGLES, GROUPS, AGREEMENTS, WINDOW>,
) -> Option<[AgreementSlot<'id>; AGREEMENTS]> {
    match data {
        Data::Stopping(running) => running.agreements.slots_home().ok(),
        Data::Stopped | Data::Flowing(_) => None,
    }
}

/// The data's rights and parts while it runs.
struct Running<
    'id,
    F: Form,
    K: Ieee80211Capabilities,
    T,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> {
    form: F::Running<'id, K>,
    /// The controller's single places, which the engine does not lend.
    singles: [SinglePlace<'id>; SINGLES],
    interface: Interface<'id, T>,
    groups: [GroupSlot<'id>; GROUPS],
    /// The receive agreements and the reorder: a station's, which phase B
    /// alone admits; an access point's banks stay free.
    agreements: Agreements<'id, K, AGREEMENTS, WINDOW>,
    /// The received units the engine holds for the network.
    receive: Receive<'id>,
}

/// The data's state.
enum Data<
    'id,
    F: Form,
    K: Ieee80211Capabilities,
    T,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> {
    /// The installation machine holds the rights.
    Stopped,
    /// The data flows.
    Flowing(Running<'id, F, K, T, SINGLES, GROUPS, AGREEMENTS, WINDOW>),
    /// Asked to stop: every link ended; what is on the air or in the gate's
    /// closing ends first.
    Stopping(Running<'id, F, K, T, SINGLES, GROUPS, AGREEMENTS, WINDOW>),
}

/// A link's state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Carrying {
    /// Its data goes.
    Carrying,
    /// Its data waits: the controller holds the link.
    Held,
}

/// Whether a TID's intake is held for a number the controller names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Hold {
    Nothing,
    /// The TID was named: its number is told once the port's places hold
    /// nothing of it, so nothing of it is below the number.
    Draining(WmmUserPriority),
    /// Its number was told: it waits for the agreement's start on it, or
    /// for the controller's release.
    Named(WmmUserPriority, SequenceNumber),
}

impl Hold {
    /// The TID whose intake waits.
    const fn tid(self) -> Option<WmmUserPriority> {
        match self {
            Self::Nothing => None,
            Self::Draining(tid) | Self::Named(tid, _) => Some(tid),
        }
    }
}

/// A linked peer: its link, its state, and the BlockAckReqs owed it.
struct Linked<E> {
    link: E,
    carrying: Carrying,
    hold: Hold,
    /// A BlockAckReq owed a TID, to the window's head.
    owed: [Option<SequenceNumber>; 8],
}

/// One entry of the peer table.
enum Entry<E> {
    Free,
    Linked(Linked<E>),
}

/// The sooner of two rests: a round before any wake, else the earlier
/// instant, a wake with none being the latest.
fn earliest(first: Rest, second: Rest) -> Rest {
    match (first, second) {
        (Rest::Round, _) | (_, Rest::Round) => Rest::Round,
        (Rest::Wake(Some(a)), Rest::Wake(Some(b))) => Rest::Wake(Some(a.min(b))),
        (Rest::Wake(Some(at)), Rest::Wake(None)) | (Rest::Wake(None), Rest::Wake(Some(at))) => {
            Rest::Wake(Some(at))
        }
        (Rest::Wake(None), Rest::Wake(None)) => Rest::Wake(None),
    }
}

/// What is wrong with an engine over `staging` places with the receive
/// window `window` under a capability of `max_window`, if anything: a port
/// stages into at most the contract's places; the window is 1 to the
/// capability; the reorder's frames (the window) leave a staging place free.
const fn engine_refusal(staging: usize, window: usize, max_window: u16) -> Option<&'static str> {
    if staging > PLACES {
        Some("a port stages into at most the contract's PLACES")
    } else if window == 0 || window > max_window as usize {
        Some("the receive window is 1 to the capabilities' rx_block_ack_max_window")
    } else if window >= staging {
        Some("the receive window is below the port's staging places: WINDOW < STAGING")
    } else {
        None
    }
}

/// The receive half's policy values, the composition's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceivePolicy {
    /// How long the network may hold the ring back: a pressure episode's
    /// bound from its first wait.
    pub late: Duration,
    /// How long a receive agreement's reorder holds a hole open before it
    /// gives the missing frames up.
    pub gap: Duration,
}

/// The engine of one interface: `PEERS` peers, a pool of `POOL` MSDUs, the
/// data's rights of a layout of `SINGLES` single places of its controller,
/// `GROUPS` group key slots and `AGREEMENTS` receive agreements, and the
/// receive window `WINDOW` (16 unless the composition names another, from 1
/// to the port's `rx_block_ack_max_window`; the reorder holds a window of
/// frames, `WINDOW < STAGING`).
pub struct Engine<
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize = RX_BLOCK_ACK_WINDOW,
> {
    /// The MSDUs a section takes at most.
    budget: u8,
    /// The receive half's policy values.
    policy: ReceivePolicy,
    /// The links made: a link's generation, which its receive agreements
    /// belong to.
    links: u8,
    data: Data<'id, F, P::Caps, T, SINGLES, GROUPS, AGREEMENTS, WINDOW>,
    /// Linked only while the data flows: a stop ends every link.
    entries: [Entry<F::Entry<P::Link, T>>; PEERS],
    queues: TxQueues<P::Memory, POOL, PEERS>,
}

/// The seal of the forms' halves: only this crate makes one, so only the
/// engine's sections call them.
#[derive(Debug)]
pub struct Seal(());

/// The form-specific half of the sections, which only the engine calls
/// ([`Seal`]).
pub trait Sections<'id, P: ReceivePort<'id>> {
    /// The data's rights of the form's layout.
    type Rights;
    /// The link's transmit key.
    type Key;

    /// The data flows, with its rights.
    fn start(&mut self, _seal: Seal, rights: Self::Rights);

    /// The queue an MSDU goes into, or why it is refused.
    fn route<N: Network<P::Memory, P::Lent>>(
        &self,
        _seal: Seal,
        network: &N,
        memory: &P::Memory,
    ) -> Result<QueueId, IntakeRefusal>;

    /// Take queued MSDUs into the form's places, counting each taken
    /// against the budget in `taken`: the MSDUs that left their queues
    /// (taken, sent or refused).
    fn fill<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
        taken: &mut u8,
    ) -> u8;

    /// Put the next arena on the air: the plan that went.
    fn publish<N, R: Rates<'id, P::Caps>, E, C>(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Option<PpduPlan>;

    /// The queue the next MSDU would be taken from now.
    fn next_queue(&self, _seal: Seal) -> Option<QueueId>;

    /// Read the end of the form's air and settle it, and serve the port's
    /// transmit facts.
    fn ended<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        _seal: Seal,
        facts: TxFacts,
        tbtt: Signal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    );

    /// Poll the command that runs, through the form's sink: what ended.
    fn poll_command<N, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told<'id>>(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Option<Ending<'id, Self::Key>>;

    /// The link's transmit key changed: what the form holds unpublished,
    /// written under the old one, goes home first.
    fn keyed<N: Network<P::Memory, P::Lent>, E: EventSink<DataEvent>>(
        &mut self,
        _seal: Seal,
        port: &mut P,
        network: &mut N,
        events: &mut E,
    );

    /// End the link of `peer`: what the form holds of it goes home.
    fn unlink_form<N: Network<P::Memory, P::Lent>, E: EventSink<DataEvent>>(
        &mut self,
        _seal: Seal,
        peer: u8,
        port: &mut P,
        network: &mut N,
        events: &mut E,
    );

    /// The rights back and the data stopped, when it is stopping, its gate
    /// shut and nothing the port's; else nothing changes.
    fn stopped(&mut self, _seal: Seal) -> Option<Self::Rights>;

    /// One receive round over the port, the form's links naming a frame's
    /// peer.
    fn receive_round<N: Network<P::Memory, P::Lent>, R, E, C: Told<'id>>(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest;

    /// Take the next number of `peer`'s link's own space.
    fn own_number(&mut self, _seal: Seal, peer: u8) -> Option<SequenceNumber>;

    /// Name `tid` of `peer`'s link: its intake waits from now.
    ///
    /// # Errors
    ///
    /// The link has no sequence space per TID; nothing changed.
    fn name_tid(&mut self, _seal: Seal, peer: u8, tid: WmmUserPriority) -> Result<(), NoSpace>;

    /// Tell each draining TID whose port's places hold nothing of it its
    /// number ([`Told::tid_named`]).
    fn tell_drained<C: Told<'id>>(&mut self, _seal: Seal, told: &mut C);
}

/// The engine type of a handle's methods.
macro_rules! engine {
    () => {
        Engine<'id, P, F, T, PEERS, POOL, SINGLES, GROUPS, AGREEMENTS, WINDOW>
    };
}

/// A station's engine type.
macro_rules! station {
    () => {
        Engine<'id, P, Arenas, T, 1, POOL, SINGLES, GROUPS, AGREEMENTS, WINDOW>
    };
}

/// An access point's engine type.
macro_rules! access_point {
    () => {
        Engine<'id, P, Singles, T, PEERS, POOL, SINGLES, GROUPS, AGREEMENTS, WINDOW>
    };
}

impl<
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> engine!()
{
    /// The engine's relations, checked where it is built: a port stages
    /// into at most the contract's places; the receive window is 1 to the
    /// port's `rx_block_ack_max_window`, and the reorder's frames (the
    /// window) leave a staging place free (`WINDOW <= FRAMES < STAGING`).
    pub const GEOMETRY: () = {
        if let Some(refusal) =
            engine_refusal(P::STAGING, WINDOW, P::Caps::CAPS.rx_block_ack_max_window)
        {
            panic!("{}", refusal);
        }
    };

    /// The engine of stopped data, taking at most `budget` MSDUs a section
    /// (at least one), its receive half under `policy`: the composition's
    /// policy values. Built in its place, a `const` item or a placed
    /// initializer.
    pub const fn new(budget: u8, policy: ReceivePolicy) -> Self {
        let () = Self::GEOMETRY;
        Self {
            budget: if budget == 0 { 1 } else { budget },
            policy,
            links: 0,
            data: Data::Stopped,
            entries: [const { Entry::Free }; PEERS],
            queues: TxQueues::new(),
        }
    }

    /// The data's place: what it is now, and the transitions it offers.
    pub fn data(&mut self) -> DataPlace<'_, Self> {
        match self.data {
            Data::Stopped => DataPlace::Stopped(StoppedData(self)),
            Data::Flowing(_) => DataPlace::Flowing(FlowingData(self)),
            Data::Stopping(_) => DataPlace::Stopping(StoppingData(self)),
        }
    }

    /// `peer`'s linked entry, while the data flows.
    fn linked_mut(&mut self, peer: u8) -> Option<&mut Linked<F::Entry<P::Link, T>>> {
        match (&self.data, self.entries.get_mut(usize::from(peer))) {
            (Data::Flowing(_), Some(Entry::Linked(linked))) => Some(linked),
            _ => None,
        }
    }

    /// The interface's command place, while the data runs.
    const fn interface_mut(&mut self) -> Option<&mut Interface<'id, T>> {
        match &mut self.data {
            Data::Flowing(running) | Data::Stopping(running) => Some(&mut running.interface),
            Data::Stopped => None,
        }
    }

    /// Whether the command place admits the intake: a key's command and the
    /// gate's closing stop it.
    const fn admits_intake(&self) -> bool {
        matches!(
            &self.data,
            Data::Flowing(Running {
                interface: Interface::Idle { .. } | Interface::Agreeing { .. },
                ..
            })
        )
    }

    /// The received units the engine holds: waiting for the network, and
    /// retained by the reorder, for the crate's search.
    #[cfg(test)]
    pub(crate) fn rx_held(&mut self) -> (usize, usize) {
        match &mut self.data {
            Data::Flowing(running) | Data::Stopping(running) => (
                running.receive.waiting(),
                running.agreements.view().retained(),
            ),
            Data::Stopped => (0, 0),
        }
    }

    /// The MSDUs the queues hold.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.queues.count()
    }

    /// Begin the gate's closing, when it is open and no command runs:
    /// nothing new goes from now.
    fn close_gate<N, R, E, C>(&mut self, turn: &mut Turn<'_, P, N, R, E, C>) {
        let Some(interface) = self.interface_mut() else {
            return;
        };
        let Interface::Idle {
            gate: Gate::Open(_),
            ..
        } = interface
        else {
            return;
        };
        *interface = match core::mem::replace(interface, Interface::InTransition) {
            Interface::Idle {
                gate: Gate::Open(open),
                place,
                key,
            } => Interface::Closing {
                command: turn.port.begin_close(place, CloseGate(open), turn.now),
                key,
            },
            // Checked open and idle above: this does not happen. Should it,
            // the interface keeps what it was.
            other => other,
        };
    }

    /// Begin a key's command toward `target`, when no command runs: the
    /// installed key's removal (then `target`), or `target`'s installation
    /// in the empty slot; [`Begun::Busy`] with the target back while a
    /// command runs.
    fn begin_key<N, R, E, C>(
        &mut self,
        target: KeyTarget<T>,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Begun<T> {
        let Some(interface) = self.interface_mut() else {
            return Begun::Busy(target);
        };
        let Interface::Idle { .. } = interface else {
            return Begun::Busy(target);
        };
        let (next, begun) = match core::mem::replace(interface, Interface::InTransition) {
            Interface::Idle {
                gate,
                place,
                key: StationKey::Installed(installed, old),
            } => (
                Interface::Keying {
                    gate,
                    command: turn.port.begin_pairwise(
                        place,
                        PairwiseCommand::Remove(installed),
                        turn.now,
                    ),
                    step: KeyStep::Removing { old, then: target },
                },
                Begun::Started,
            ),
            Interface::Idle {
                gate,
                place,
                key: StationKey::Empty(slot),
            } => match target {
                KeyTarget::Install(KeyChange {
                    peer,
                    key,
                    spp,
                    transmit,
                }) => (
                    Interface::Keying {
                        gate,
                        command: turn.port.begin_pairwise(
                            place,
                            PairwiseCommand::Install {
                                slot,
                                peer,
                                key,
                                spp,
                            },
                            turn.now,
                        ),
                        step: KeyStep::Installing { transmit },
                    },
                    Begun::Started,
                ),
                KeyTarget::Empty => (
                    Interface::Idle {
                        gate,
                        place,
                        key: StationKey::Empty(slot),
                    },
                    Begun::Nothing,
                ),
            },
            // Checked idle above: this does not happen. Should it, the
            // interface keeps what it was.
            other => (other, Begun::Busy(target)),
        };
        *interface = next;
        begun
    }

    /// The interface's command polled through `sink`: its end puts the
    /// command's place back with what the end returns, and says what ended.
    fn poll_interface(
        interface: &mut Interface<'id, T>,
        port: &mut P,
        now: Instant,
        sink: &mut impl PortSink<'id, P::Ack, P::Denial>,
    ) -> Option<Ending<'id, T>> {
        let (Interface::Keying { .. } | Interface::Closing { .. } | Interface::Agreeing { .. }) =
            interface
        else {
            return None;
        };
        let (next, ending) = match core::mem::replace(interface, Interface::InTransition) {
            Interface::Closing { command, key } => match port.poll_close(command, now, sink) {
                CommandPoll::Pending(command) => (Interface::Closing { command, key }, None),
                CommandPoll::Ended(place, closed) => (
                    Interface::Idle {
                        gate: Gate::Shut,
                        place,
                        key,
                    },
                    Some(Ending::GateClosed(closed)),
                ),
            },
            Interface::Keying {
                gate,
                command,
                step,
            } => match port.poll_pairwise(command, now, sink) {
                CommandPoll::Pending(command) => (
                    Interface::Keying {
                        gate,
                        command,
                        step,
                    },
                    None,
                ),
                CommandPoll::Ended(place, end) => {
                    let (key, ending) = match (end, step) {
                        (PairwiseEnd::Installed(installed), KeyStep::Installing { transmit }) => (
                            StationKey::Installed(installed, transmit),
                            Ending::Key {
                                end: KeyEnd::Installed,
                                then: KeyTarget::Empty,
                            },
                        ),
                        (PairwiseEnd::Empty(slot), KeyStep::Installing { .. }) => (
                            StationKey::Empty(slot),
                            Ending::Key {
                                end: KeyEnd::Expired,
                                then: KeyTarget::Empty,
                            },
                        ),
                        (PairwiseEnd::Empty(slot), KeyStep::Removing { then, .. }) => (
                            StationKey::Empty(slot),
                            Ending::Key {
                                end: KeyEnd::Removed,
                                then,
                            },
                        ),
                        (PairwiseEnd::Installed(installed), KeyStep::Removing { old, .. }) => (
                            // A removal ends with the slot empty; a key
                            // still installed stays so, and the controller
                            // is told it.
                            StationKey::Installed(installed, old),
                            Ending::Key {
                                end: KeyEnd::Installed,
                                then: KeyTarget::Empty,
                            },
                        ),
                    };
                    (Interface::Idle { gate, place, key }, Some(ending))
                }
            },
            Interface::Agreeing {
                gate,
                command,
                ticket,
                key,
            } => match port.poll_agreement(command, now, sink) {
                CommandPoll::Pending(command) => (
                    Interface::Agreeing {
                        gate,
                        command,
                        ticket,
                        key,
                    },
                    None,
                ),
                CommandPoll::Ended(place, end) => (
                    Interface::Idle { gate, place, key },
                    Some(Ending::Agreement { ticket, end }),
                ),
            },
            idle @ (Interface::Idle { .. } | Interface::InTransition) => (idle, None),
        };
        *interface = next;
        ending
    }
}

/// Whether a command began.
enum Begun<T> {
    Started,
    /// Nothing to begin.
    Nothing,
    /// A command runs: the target back.
    Busy(KeyTarget<T>),
}

/// What a command's end leaves for the engine to tell: made by the
/// engine's sections alone ([`Seal`]).
pub enum Ending<'id, T> {
    GateClosed(GateClosed<'id>),
    /// A receive agreement's command ended, for the reorder's `bank`.
    Agreement {
        ticket: BankTicket,
        end: AgreementEnd<'id>,
    },
    Key {
        end: KeyEnd,
        /// What the slot is to become now, after a removal.
        then: KeyTarget<T>,
    },
}

/// How a key's command ended, as the controller is told.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyEnd {
    /// The key is installed: protected data goes under it.
    Installed,
    /// Its installation expired: the slot is empty, no key; nothing
    /// protected goes, and the controller ends the link.
    Expired,
    /// The installed key is removed (the data's stop): the slot is empty.
    Removed,
}

impl<
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> engine!()
where
    Self: Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>, Key = T>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// The network has MSDUs.
    pub fn offered<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        self.intake(turn)
    }

    /// The pump's turn: a round a section owed, or a deadline.
    pub fn round<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        self.command(turn);
        self.intake(turn)
    }

    /// The port's transmit facts: the arena's end and the singles', then
    /// the command in progress, then the intake.
    pub fn transmitted<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        facts: TxFacts,
        tbtt: Signal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        self.ended(Seal(()), facts, tbtt, turn);
        self.command(turn);
        self.intake(turn)
    }

    /// The ring has units: one receive round.
    pub fn received<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let rest = self.receive_round(Seal(()), turn);
        // An agreement's command the round made owed begins, one at a time.
        self.command(turn);
        rest
    }

    /// The network gave lent units back: one receive round, which takes
    /// them first.
    pub fn returned<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let rest = self.receive_round(Seal(()), turn);
        // An agreement's command the round made owed begins, one at a time.
        self.command(turn);
        rest
    }

    /// A receive wake's instant: the pressure episode's bound or the ring's
    /// continuation. One receive round.
    pub fn due<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let rest = self.receive_round(Seal(()), turn);
        // An agreement's command the round made owed begins, one at a time.
        self.command(turn);
        rest
    }

    /// The rights back, when the data is stopping, the gate shut and
    /// nothing the port's.
    fn finish(&mut self) -> DataStop<F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>> {
        // The engine's own places are quiet: the stop's round sent back
        // every unit that waited for the network, and a stopping round takes
        // none. The units the network holds are the installation machine's
        // to wait for (`UnitsHome`).
        // `stopped` answers only for quiet air: an arena on the air is not
        // idle, a single on the air keeps its lane.
        match self.stopped(Seal(())) {
            Some(rights) => DataStop::Stopped(rights),
            None => DataStop::Going,
        }
    }

    /// End `peer`'s link: its queued MSDUs go home, its entry is free.
    fn unlink<N: Network<P::Memory, P::Lent>, E: EventSink<DataEvent>>(
        &mut self,
        peer: u8,
        port: &mut P,
        network: &mut N,
        events: &mut E,
    ) {
        while let Some(msdu) = self.queues.pop_peer(peer) {
            report(events, || DataEvent::DataLost(DataLoss::LeftBehind));
            network.home(msdu.memory);
        }
        // Its accounts start anew at its next link (`link`): what is still
        // on the air charges them after this.
        self.unlink_form(Seal(()), peer, port, network, events);
        if let Some(entry) = self.entries.get_mut(usize::from(peer)) {
            *entry = Entry::Free;
        }
    }

    /// Link the free `peer`: its entry holds `link`.
    fn link<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        peer: u8,
        link: F::Entry<P::Link, T>,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        self.links = self.links.wrapping_add(1);
        if let Some(entry) = self.entries.get_mut(usize::from(peer)) {
            *entry = Entry::Linked(Linked {
                link,
                carrying: Carrying::Carrying,
                hold: Hold::Nothing,
                owed: [None; 8],
            });
        }
        // The peer's accounts start anew: what was on the air when its last
        // link ended charged them after the unlink.
        self.queues.reset_peer(peer);
        self.intake(turn)
    }

    /// What the network offers, into the queues while the pool has room;
    /// what has no link goes back at once. Whether it moved anything.
    fn drain<N: Network<P::Memory, P::Lent>, R, E: EventSink<DataEvent>, C>(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> bool {
        let mut moved = false;
        while self.queues.has_room() {
            let Some(memory) = turn.network.take() else {
                break;
            };
            moved = true;
            match self.route(Seal(()), turn.network, &memory) {
                Ok(queue) => {
                    let length = turn.network.length(&memory);
                    let priority = turn.network.priority(&memory);
                    if let Err(memory) = self.queues.push_parts(queue, memory, length, priority) {
                        // The pool had room: this does not happen.
                        report(turn.events, || DataEvent::DataLost(DataLoss::Overflow));
                        turn.network.home(memory);
                    }
                }
                Err(reason) => {
                    report(turn.events, || DataEvent::MsduRefused(reason));
                    turn.network.home(memory);
                }
            }
        }
        moved
    }

    /// The command that runs, polled; what ended told; a stopping data's
    /// next command begun.
    fn command<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) {
        if let Some(ending) = self.poll_command(Seal(()), turn) {
            match ending {
                Ending::GateClosed(closed) => turn.told.gate_closed(closed),
                Ending::Agreement { ticket, end } => self.agreement_ended(ticket, end, turn),
                Ending::Key { end, then } => {
                    self.keyed(Seal(()), turn.port, turn.network, turn.events);
                    // A rekey's removal is followed by its installation at
                    // once: the intake stands between them.
                    // A stop ended the link the rekey was for: its key is
                    // not installed, and the removal is told.
                    let begun = match (then, &self.data) {
                        (KeyTarget::Install(change), Data::Flowing(_)) => {
                            self.begin_key(KeyTarget::Install(change), turn)
                        }
                        (KeyTarget::Install(_) | KeyTarget::Empty, _) => Begun::Nothing,
                    };
                    if !matches!(begun, Begun::Started) {
                        turn.told.key_ended(end);
                    }
                    self.queues.unblock();
                }
            }
        }
        if let Data::Stopping(_) = self.data {
            self.advance_stop(turn);
        } else {
            self.begin_agreement(turn);
        }
    }

    /// A receive agreement's command ended: the reorder's bank is told, and
    /// a reply it now owes is told the controller.
    fn agreement_ended<N, R, E, C: Told<'id>>(
        &mut self,
        ticket: BankTicket,
        end: AgreementEnd<'id>,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) {
        let (Data::Flowing(running) | Data::Stopping(running)) = &mut self.data else {
            return;
        };
        let mut agreements = running.agreements.view();
        match end {
            AgreementEnd::Added(agreement) => agreements.added(ticket, agreement, &mut ()),
            AgreementEnd::Empty(slot) => agreements.emptied(ticket, slot),
        }
        if agreements.next_reply().is_some() {
            turn.told.reply_owed();
        }
        turn.told.agreement_ended();
    }

    /// Begin the command a receive agreement's bank owes, when no command
    /// runs: an addition, or an end.
    fn begin_agreement<N, R, E, C>(&mut self, turn: &mut Turn<'_, P, N, R, E, C>) {
        let (Data::Flowing(running) | Data::Stopping(running)) = &mut self.data else {
            return;
        };
        let Running {
            interface,
            agreements,
            ..
        } = running;
        let Interface::Idle { .. } = interface else {
            return;
        };
        let Some(owed) = agreements.view().next_command() else {
            return;
        };
        *interface = match core::mem::replace(interface, Interface::InTransition) {
            Interface::Idle { gate, place, key } => {
                let (ticket, command) = match owed {
                    BankCommand::Add {
                        ticket,
                        slot,
                        agreement,
                    } => (ticket, AgreementCommand::Add { slot, agreement }),
                    BankCommand::End { ticket, running } => {
                        (ticket, AgreementCommand::End(running))
                    }
                };
                Interface::Agreeing {
                    gate,
                    command: turn.port.begin_agreement(place, command, turn.now),
                    ticket,
                    key,
                }
            }
            // Checked idle above: this does not happen. Should it, the
            // interface keeps what it was, and the bank its command, which
            // goes back to it.
            other => {
                match owed {
                    BankCommand::Add { ticket, slot, .. } => {
                        agreements.view().emptied(ticket, slot);
                    }
                    BankCommand::End { ticket, running } => {
                        agreements.view().owe_end(ticket, running);
                    }
                }
                other
            }
        };
    }

    /// The stopping data's next command, when none runs: the installed
    /// key's removal, then the open gate's closing.
    fn advance_stop<N, R, E, C>(&mut self, turn: &mut Turn<'_, P, N, R, E, C>) {
        // The receive agreements end first, then the installed key, then
        // the gate.
        self.begin_agreement(turn);
        if let Begun::Nothing = self.begin_key(KeyTarget::Empty, turn) {
            self.close_gate(turn);
        }
    }

    /// The intake: drain, fill, publish, again while it moved something,
    /// within one budget. A round is owed when the budget ran out with more
    /// to take.
    fn intake<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        self.tell_drained(Seal(()), turn.told);
        let mut taken = 0;
        loop {
            let drained = self.drain(turn);
            let filled = self.fill(Seal(()), turn, &mut taken);
            let published = self.publish(Seal(()), turn);
            if !drained && filled == 0 && published.is_none() {
                break;
            }
        }
        if taken >= self.budget && self.next_queue(Seal(())).is_some() {
            Rest::Round
        } else {
            Rest::Wake(None)
        }
    }
}
/// What the data is now: a view of the engine's own place, lent for the
/// step.
pub enum DataPlace<'a, E> {
    /// The installation machine holds the rights: the data flows when it
    /// hands them over.
    Stopped(StoppedData<'a, E>),
    /// The data flows: its peers and its gate.
    Flowing(FlowingData<'a, E>),
    /// The data's stop runs: what is still the port's ends first.
    Stopping(StoppingData<'a, E>),
}

/// The stopped data's handle.
pub struct StoppedData<'a, E>(&'a mut E);

/// The flowing data's handle.
pub struct FlowingData<'a, E>(&'a mut E);

/// The stopping data's handle.
pub struct StoppingData<'a, E>(&'a mut E);

/// What a peer's place is now.
pub enum PeerPlace<'a, E> {
    /// It has no link.
    Free(FreePeer<'a, E>),
    /// It has one.
    Linked(LinkedPeer<'a, E>),
}

/// A free peer's handle.
pub struct FreePeer<'a, E> {
    engine: &'a mut E,
    peer: u8,
}

/// A linked peer's handle.
pub struct LinkedPeer<'a, E> {
    engine: &'a mut E,
    peer: u8,
}

/// What a linked peer's named TID is now: one TID at a time is named.
pub enum NamePlace<'a, E> {
    /// No TID is named: one may be.
    Unnamed(UnnamedTids<'a, E>),
    /// A TID was named: its intake waits, and its number is told
    /// ([`Told::tid_named`]) once the port's places hold nothing of it.
    Draining(DrainingTid<'a, E>),
    /// Its number was told: its intake waits for the agreement's start on
    /// it or for the controller's release.
    Named(NamedTid<'a, E>),
}

/// The handle of a link with no TID named.
pub struct UnnamedTids<'a, E> {
    engine: &'a mut E,
    peer: u8,
}

/// The handle of a named TID whose number is not told yet.
pub struct DrainingTid<'a, E> {
    engine: &'a mut E,
    peer: u8,
    tid: WmmUserPriority,
}

/// The handle of a named TID whose number was told.
pub struct NamedTid<'a, E> {
    engine: &'a mut E,
    peer: u8,
    tid: WmmUserPriority,
    number: SequenceNumber,
}

/// A reply the receive agreements owe, lent the controller's step.
pub struct RxReply<'a, E> {
    engine: &'a mut E,
    body: [u8; ADDBA_ACTION_BODY_LEN],
    place: ReplyPlace,
}

impl<
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> RxReply<'_, engine!()>
{
    /// The Action body to send.
    pub const fn body(&self) -> &[u8; ADDBA_ACTION_BODY_LEN] {
        &self.body
    }

    /// The reply went: it is owed no more.
    pub fn sent(self) {
        if let Data::Flowing(running) | Data::Stopping(running) = &mut self.engine.data {
            running.agreements.view().sent(self.place);
        }
    }
}

/// What the gate's place is now.
pub enum GatePlace<'a, E> {
    /// The controller holds it, closed.
    Shut(ShutGate<'a, E>),
    /// The engine holds it open.
    Open(OpenGate<'a, E>),
    /// Open while a key's command runs: nothing goes, and it offers no
    /// closing until the key's end ([`Told::key_ended`]).
    OpenKeying(KeyingGate<'a, E>),
    /// Open while a receive agreement's command runs: the data goes on,
    /// and it offers no closing until the command's end
    /// ([`Told::agreement_ended`]).
    OpenAgreeing(AgreeingGate<'a, E>),
    /// Its closing runs in the port; it ends in [`Told::gate_closed`].
    Closing(ClosingGate<'a, E>),
}

/// A shut gate's handle.
pub struct ShutGate<'a, E>(&'a mut E);

/// An open gate's handle.
pub struct OpenGate<'a, E>(&'a mut E);

/// A closing gate's handle: it offers no transition.
pub struct ClosingGate<'a, E>(PhantomData<&'a mut E>);

/// An open gate's handle while a receive agreement's command runs: it
/// offers no transition.
pub struct AgreeingGate<'a, E>(PhantomData<&'a mut E>);

/// An open gate's handle while a key's command runs: it offers no
/// transition.
pub struct KeyingGate<'a, E>(PhantomData<&'a mut E>);

/// What the station's key place is now.
pub enum KeyPlace<'a, E> {
    /// No command runs: a key may be installed.
    Idle(IdleKey<'a, E>),
    /// A key's command runs; it ends in [`Told::key_ended`].
    Changing(ChangingKey<'a, E>),
    /// A receive agreement's command runs; it ends in
    /// [`Told::agreement_ended`].
    Agreeing(AgreeingKey<'a, E>),
    /// Another command runs (the gate's closing).
    Busy(BusyKey<'a, E>),
}

/// The idle key place's handle.
pub struct IdleKey<'a, E>(&'a mut E);

/// A changing key's handle: it offers no transition.
pub struct ChangingKey<'a, E>(PhantomData<&'a mut E>);

/// A key place's handle while a receive agreement's command runs: it offers
/// no transition.
pub struct AgreeingKey<'a, E>(PhantomData<&'a mut E>);

/// A busy key place's handle: it offers no transition.
pub struct BusyKey<'a, E>(PhantomData<&'a mut E>);

impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> FlowingData<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>, Key = T>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// The receive refusals counted so far, by reason.
    pub fn refusals(&self) -> RxRefusals {
        match &self.0.data {
            Data::Flowing(running) | Data::Stopping(running) => running.receive.refusals(),
            Data::Stopped => RxRefusals::default(),
        }
    }

    /// `peer`'s place.
    pub fn peer(&mut self, peer: Peer<PEERS>) -> PeerPlace<'_, engine!()> {
        let engine = &mut *self.0;
        if let Some(Entry::Linked(_)) = engine.entries.get(usize::from(peer.0)) {
            PeerPlace::Linked(LinkedPeer {
                engine,
                peer: peer.0,
            })
        } else {
            PeerPlace::Free(FreePeer {
                engine,
                peer: peer.0,
            })
        }
    }

    /// The gate's place.
    pub fn gate(&mut self) -> GatePlace<'_, engine!()> {
        let engine = &mut *self.0;
        match engine.interface_mut() {
            Some(Interface::Idle {
                gate: Gate::Open(_),
                ..
            }) => GatePlace::Open(OpenGate(engine)),
            Some(Interface::Keying {
                gate: Gate::Open(_),
                ..
            }) => GatePlace::OpenKeying(KeyingGate(PhantomData)),
            Some(Interface::Agreeing {
                gate: Gate::Open(_),
                ..
            }) => GatePlace::OpenAgreeing(AgreeingGate(PhantomData)),
            Some(Interface::Closing { .. } | Interface::InTransition) => {
                GatePlace::Closing(ClosingGate(PhantomData))
            }
            Some(
                Interface::Idle {
                    gate: Gate::Shut, ..
                }
                | Interface::Keying {
                    gate: Gate::Shut, ..
                }
                | Interface::Agreeing {
                    gate: Gate::Shut, ..
                },
            )
            | None => GatePlace::Shut(ShutGate(engine)),
        }
    }

    /// Stop the data, as the installation machine asks: every link ends,
    /// its queued and unpublished data goes home, the installed key is
    /// removed and an open gate closes, one command after the other. The
    /// rights come back once nothing is the port's; until
    /// then the answer is [`DataStop::Going`], and the machine asks again
    /// ([`StoppingData::poll`]).
    pub fn stop<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> DataStop<F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>> {
        let engine = self.0;
        for peer in 0..PEERS {
            if let Ok(peer) = u8::try_from(peer)
                && let Some(Entry::Linked(_)) = engine.entries.get(usize::from(peer))
            {
                engine.unlink(peer, turn.port, turn.network, turn.events);
            }
        }
        engine.data = match core::mem::replace(&mut engine.data, Data::Stopped) {
            // The data's parts move into the stopping variant: a copy of
            // the running data, once a stop.
            Data::Flowing(running) => Data::Stopping(running),
            other => other,
        };
        // What waited for the network, what the reorder retains and what the
        // ring holds go back. A stopping round leaves nothing waiting; what
        // the ring stages after it goes back in the round of the machine's
        // next poll.
        let _: Rest = engine.receive_round(Seal(()), turn);
        engine.advance_stop(turn);
        // With no link, what the network still offers goes back.
        engine.drain(turn);
        engine.finish()
    }
}

impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> StoppingData<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>, Key = T>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// The machine asks the data's stop again: the gate's closing is
    /// polled, and the rights come back once nothing is the port's.
    pub fn poll<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> DataStop<F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>> {
        let engine = self.0;
        engine.command(turn);
        // The ring's next units go back unread; the machine polls again
        // until the rights come home.
        let _: Rest = engine.receive_round(Seal(()), turn);
        engine.finish()
    }
}

impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> LinkedPeer<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>, Key = T>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// End the link: its queued data goes home.
    pub fn unlink<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let Self { engine, peer } = self;
        engine.unlink(peer, turn.port, turn.network, turn.events);
        // The link's receive agreements end: what the reorder retains goes
        // back, and their ends begin.
        let received = engine.receive_round(Seal(()), turn);
        engine.command(turn);
        earliest(received, engine.intake(turn))
    }

    /// Change the linked entry and run the intake. The handle borrows the
    /// engine, so the peer is linked while the handle lives.
    fn change<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
        change: impl FnOnce(&mut Linked<F::Entry<P::Link, T>>),
    ) -> Rest {
        if let Some(linked) = self.engine.linked_mut(self.peer) {
            change(linked);
        }
        self.engine.queues.unblock();
        self.engine.intake(turn)
    }

    /// Hold the link's data.
    pub fn hold<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        self.change(turn, |linked| linked.carrying = Carrying::Held)
    }

    /// Let it go again.
    pub fn release<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        self.change(turn, |linked| linked.carrying = Carrying::Carrying)
    }

    /// Take the next number of the link's own space, for a frame of the
    /// controller that carries it (management, EAPOL, a non-QoS BSS's data).
    /// `None` never for a linked peer's handle, which borrows the engine.
    pub fn own_number(&mut self) -> Option<SequenceNumber> {
        self.engine.own_number(Seal(()), self.peer)
    }

    /// The place of the link's named TID.
    pub fn naming(&mut self) -> NamePlace<'_, engine!()> {
        let hold = self
            .engine
            .linked_mut(self.peer)
            .map_or(Hold::Nothing, |linked| linked.hold);
        let (engine, peer) = (&mut *self.engine, self.peer);
        match hold {
            Hold::Nothing => NamePlace::Unnamed(UnnamedTids { engine, peer }),
            Hold::Draining(tid) => NamePlace::Draining(DrainingTid { engine, peer, tid }),
            Hold::Named(tid, number) => NamePlace::Named(NamedTid {
                engine,
                peer,
                tid,
                number,
            }),
        }
    }

    /// The BlockAckReq owed for `tid`, to its window's head.
    pub fn bar_owed(&mut self, tid: WmmUserPriority) -> Option<SequenceNumber> {
        self.engine
            .linked_mut(self.peer)
            .and_then(|linked| *tid.select(&linked.owed))
    }

    /// The BlockAckReq owed for `tid` went: it is owed no more.
    pub fn bar_sent(&mut self, tid: WmmUserPriority) {
        if let Some(linked) = self.engine.linked_mut(self.peer) {
            *tid.select_mut(&mut linked.owed) = None;
        }
    }

    /// The reply the receive agreements owe the peer (an ADDBA Response
    /// or a DELBA body), when they owe one: the controller sends it, then
    /// says so ([`RxReply::sent`]).
    pub fn rx_reply(&mut self) -> Option<RxReply<'_, engine!()>> {
        let (Data::Flowing(running) | Data::Stopping(running)) = &mut self.engine.data else {
            return None;
        };
        let reply = running.agreements.view().next_reply()?;
        Some(RxReply {
            engine: &mut *self.engine,
            body: reply.body,
            place: reply.place,
        })
    }

    /// The link's state, for the crate's search.
    #[cfg(test)]
    pub(crate) fn is_held(&mut self) -> bool {
        self.engine
            .linked_mut(self.peer)
            .is_some_and(|linked| linked.carrying == Carrying::Held)
    }
}

impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> UnnamedTids<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>, Key = T>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// Name `tid`: its intake waits from now, and its number is told
    /// ([`Told::tid_named`]) once the port's places hold nothing of it, at
    /// once when they hold nothing now. The number is not taken: the TID's
    /// first data MPDU after carries it.
    ///
    /// # Errors
    ///
    /// The link has no TID's space: its peer did not negotiate QoS. Nothing
    /// changed.
    pub fn name<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        tid: WmmUserPriority,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Result<Rest, NoSpace> {
        let Self { engine, peer } = self;
        engine.name_tid(Seal(()), peer, tid)?;
        Ok(engine.intake(turn))
    }
}
impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> DrainingTid<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>, Key = T>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// The named TID.
    pub const fn tid(&self) -> WmmUserPriority {
        self.tid
    }

    /// The controller releases the name before its number was told: the
    /// TID's intake goes on.
    pub fn release<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        unhold(self.engine, self.peer, turn)
    }
}
impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> NamedTid<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>, Key = T>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// The named TID.
    pub const fn tid(&self) -> WmmUserPriority {
        self.tid
    }

    /// The number told: the one its next data MPDU carries.
    pub const fn number(&self) -> SequenceNumber {
        self.number
    }

    /// The controller releases the name (the exchange that carried the
    /// number was refused or failed): the TID's intake goes on, its first
    /// data MPDU carrying the number.
    pub fn release<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        unhold(self.engine, self.peer, turn)
    }
}

/// A name's hold ends: the TID's intake goes on.
fn unhold<
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    N: Network<P::Memory, P::Lent>,
    R: Rates<'id, P::Caps>,
    E: EventSink<DataEvent>,
    C: Told<'id>,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
>(
    engine: &mut engine!(),
    peer: u8,
    turn: &mut Turn<'_, P, N, R, E, C>,
) -> Rest
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>, Key = T>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    if let Some(linked) = engine.linked_mut(peer) {
        linked.hold = Hold::Nothing;
    }
    engine.queues.unblock();
    engine.intake(turn)
}

impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> ShutGate<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>, Key = T>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// Open the closed gate the controller lends: arenas and singles go.
    pub fn open<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        gate: GateClosed<'id>,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let engine = self.0;
        if let Some(
            Interface::Idle { gate: place, .. }
            | Interface::Keying { gate: place, .. }
            | Interface::Agreeing { gate: place, .. },
        ) = engine.interface_mut()
            && let Gate::Shut = place
        {
            *place = Gate::Open(turn.port.open_gate(gate));
        }
        engine.intake(turn)
    }
}

impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> OpenGate<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>, Key = T>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// Close the gate: nothing new goes, and the port closes it once its
    /// air is quiet; the closed gate then goes to the controller
    /// ([`Told::gate_closed`]).
    pub fn close<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let engine = self.0;
        engine.close_gate(turn);
        engine.intake(turn)
    }
}

impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> StoppedData<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>, Key = T>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// The data flows: the installation machine hands its rights over.
    pub fn flows(
        self,
        rights: F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>,
    ) -> FlowingData<'a, engine!()> {
        let engine = self.0;
        engine.start(Seal(()), rights);
        FlowingData(engine)
    }
}

impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    T: TxKey,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> FreePeer<'a, station!()>
where
    P::Ack: Copy,
    P::BlockAck: Copy,
    P::Denial: Copy,
{
    /// Link the access point: the plane takes the link's data state, the
    /// entry the port's link and the numbers. Its data goes while the gate
    /// is open.
    pub fn link<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        link: StationLink<P::Link>,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let Self { engine, peer } = self;
        let StationLink {
            data,
            port,
            numbers,
        } = link;
        if let Data::Flowing(running) = &mut engine.data {
            // The link's data state into the plane's variant: a copy, once
            // an association.
            running.form.plane.link(data);
        }
        engine.link(peer, StationEntry { port, numbers }, turn)
    }
}

impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    T: TxKey,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> FreePeer<'a, access_point!()>
where
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// Link the client.
    pub fn link<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        link: ClientLink,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let Self { engine, peer } = self;
        engine.link(peer, link, turn)
    }
}

/// The station's key: its link's pairwise key, changed by a command.
impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    T: TxKey,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> LinkedPeer<'a, station!()>
where
    P::Ack: Copy,
    P::BlockAck: Copy,
    P::Denial: Copy,
{
    /// The key's place.
    pub fn key(&mut self) -> KeyPlace<'_, station!()> {
        let engine = &mut *self.engine;
        match engine.interface_mut() {
            Some(Interface::Idle { .. }) => KeyPlace::Idle(IdleKey(engine)),
            Some(Interface::Keying { .. }) => KeyPlace::Changing(ChangingKey(PhantomData)),
            Some(Interface::Agreeing { .. }) => KeyPlace::Agreeing(AgreeingKey(PhantomData)),
            Some(Interface::Closing { .. } | Interface::InTransition) | None => {
                KeyPlace::Busy(BusyKey(PhantomData))
            }
        }
    }
}

impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    T: TxKey,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> IdleKey<'a, station!()>
where
    P::Ack: Copy,
    P::BlockAck: Copy,
    P::Denial: Copy,
{
    /// Install `change` as the link's key: the installed key's removal
    /// first, when there is one, then its installation. The intake stands
    /// until the installation's end; then what the arenas hold unpublished
    /// goes home and the key is the link's ([`Told::key_ended`]).
    pub fn change<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        change: KeyChange<T>,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let engine = self.0;
        // The handle is an idle place's: the key's command begins.
        let _begun = engine.begin_key(KeyTarget::Install(change), turn);
        engine.intake(turn)
    }
}

/// A station's named TID begins its transmit agreement: the arenas' form
/// alone aggregates.
impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    T: TxKey,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> NamedTid<'a, station!()>
where
    P::Ack: Copy,
    P::BlockAck: Copy,
    P::Denial: Copy,
{
    /// The named TID's transmit agreement is operational with a window of
    /// `size`, 1 to 64. Its head is the number told: the ADDBA Request's
    /// Starting Sequence Number, which the peer counts from (IEEE Std
    /// 802.11-2020, 10.25.2). The name's hold ends.
    pub fn tx_agreement<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        size: u16,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let Self {
            engine,
            peer,
            tid,
            number,
        } = self;
        if let Data::Flowing(running) = &mut engine.data {
            running
                .form
                .plane
                .agreement(tid, Some(TxWindow { head: number, size }));
        }
        unhold(engine, peer, turn)
    }
}

/// The station's transmit agreements: the arenas' form alone aggregates.
impl<
    'a,
    'id,
    P: ReceivePort<'id>,
    T: TxKey,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> LinkedPeer<'a, station!()>
where
    P::Ack: Copy,
    P::BlockAck: Copy,
    P::Denial: Copy,
{
    /// The TID's transmit agreement ended.
    pub fn tx_agreement_ended<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        tid: WmmUserPriority,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let engine = &mut *self.engine;
        if let Data::Flowing(running) = &mut engine.data {
            running.form.plane.agreement(tid, None);
        }
        engine.queues.unblock();
        engine.intake(turn)
    }
}

impl<
    'id,
    P: ReceivePort<'id>,
    T,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> station!()
{
    /// The port's A-MPDU limits at the rate the aggregate rate selection
    /// gives now: the arenas' fill follows them.
    pub fn aggregate_rate(&mut self, rate: &crate::plane::AggregateRate) {
        if let Data::Flowing(running) | Data::Stopping(running) = &mut self.data {
            running.form.plane.aggregate_rate(Some(*rate));
        }
    }
}

/// The right of `category` among rights in `QUEUES`' order.
fn in_queues_order<T>(rights: &mut [T; 4], category: WmmAccessCategory) -> &mut T {
    let [voice, video, best_effort, background] = rights;
    match category {
        WmmAccessCategory::Voice => voice,
        WmmAccessCategory::Video => video,
        WmmAccessCategory::BestEffort => best_effort,
        WmmAccessCategory::Background => background,
    }
}

/// The sink of a station's port: its data lends no single and no queue,
/// and the controller's singles are B4's; the TBTT goes to the controller.
struct StationEnds<'a, 'id, C, const SINGLES: usize> {
    singles: &'a mut [SinglePlace<'id>; SINGLES],
    queues: &'a mut [QueueRight<'id>; 4],
    told: &'a mut C,
}

impl<'id, A, D, C: Told<'id>, const SINGLES: usize> PortSink<'id, A, D>
    for StationEnds<'_, 'id, C, SINGLES>
{
    fn single_ended(
        &mut self,
        _outcome: SingleOutcome<A, D>,
        place: SinglePlace<'id>,
        queue: QueueRight<'id>,
    ) {
        // The port gives back only what it was lent, and the station's data
        // lends nothing, so this end does not come. Each right is its
        // installation's one: it is written over its own index.
        if let Some(home) = self.singles.get_mut(usize::from(place.index())) {
            *home = place;
        }
        let category = queue.category();
        *in_queues_order(self.queues, category) = queue;
    }

    fn tbtt(&mut self, event: TbttEvent) {
        self.told.tbtt(event);
    }
}

/// A station: its access point's link in the plane.
impl<
    'id,
    P: ReceivePort<'id>,
    T: TxKey,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> Sections<'id, P> for station!()
where
    P::Ack: Copy,
    P::BlockAck: Copy,
    P::Denial: Copy,
{
    type Rights = DataRights<'id, SINGLES, GROUPS, AGREEMENTS, 0>;
    type Key = T;

    fn start(&mut self, _seal: Seal, rights: Self::Rights) {
        let DataRights {
            arenas,
            singles,
            data: [],
            queues,
            command,
            pairwise,
            groups,
            agreements,
        } = rights;
        // The plane is built with the arenas into the flowing variant: a
        // copy, once the data's start.
        self.data = Data::Flowing(Running {
            form: StationRunning {
                plane: DataPlane::new(arenas),
                queues,
            },
            singles,
            interface: Interface::Idle {
                gate: Gate::Shut,
                place: command,
                key: StationKey::Empty(pairwise),
            },
            groups,
            agreements: RxBlockAckStorage::new(agreements, self.policy.gap),
            receive: Receive::new(self.policy.late),
        });
    }

    fn route<N: Network<P::Memory, P::Lent>>(
        &self,
        _seal: Seal,
        network: &N,
        memory: &P::Memory,
    ) -> Result<QueueId, IntakeRefusal> {
        let (Data::Flowing(running), [Entry::Linked(_)]) = (&self.data, &self.entries) else {
            return Err(IntakeRefusal::NoLink);
        };
        running
            .form
            .plane
            .tid(network.priority(memory))
            .map(QueueId::of_tid)
            .ok_or(IntakeRefusal::NoAdmittedAccessCategory)
    }

    fn fill<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
        taken: &mut u8,
    ) -> u8 {
        if !self.admits_intake() {
            return 0;
        }
        let Self {
            data: Data::Flowing(running),
            entries:
                [
                    Entry::Linked(Linked {
                        link,
                        carrying: Carrying::Carrying,
                        hold,
                        ..
                    }),
                ],
            queues,
            budget,
            policy: _,
            links: _,
        } = self
        else {
            return 0;
        };
        let Running {
            form: StationRunning { plane, .. },
            interface,
            ..
        } = running;
        // The link's transmit key exists while the port's key is
        // installed; a key's command stops the intake.
        let key = match interface {
            Interface::Idle {
                key: StationKey::Installed(_, transmit),
                ..
            }
            | Interface::Agreeing {
                key: StationKey::Installed(_, transmit),
                ..
            } => Some(&*transmit),
            _ => None,
        };
        let hold = hold.tid().map(QueueId::of_tid);
        let mut moved: u8 = 0;
        while *taken < *budget {
            let queue = match plane.standby_tid() {
                // The standby's TID goes on, or nothing does.
                Some(tid) => {
                    let queue = QueueId::of_tid(tid);
                    if !queues.ready(queue) || Some(queue) == hold {
                        break;
                    }
                    queue
                }
                None => match queues.select(hold) {
                    Some(queue) => queue,
                    None => break,
                },
            };
            let Some(msdu) = queues.pop(queue) else {
                break;
            };
            *taken = taken.saturating_add(1);
            match plane.intake(
                turn.port,
                &link.port,
                msdu,
                &mut link.numbers,
                key,
                turn.now,
            ) {
                Intake::Taken(_) => moved = moved.saturating_add(1),
                Intake::Wait(msdu, why) => {
                    if let Err(msdu) = queues.push_front(queue, msdu) {
                        report(turn.events, || DataEvent::DataLost(DataLoss::Overflow));
                        turn.network.home(msdu.memory);
                    }
                    match why {
                        // Only this TID waits for its window.
                        Waits::Window => queues.block(queue),
                        Waits::Arena | Waits::Standby => break,
                    }
                }
                Intake::Refused { memory, reason } => {
                    moved = moved.saturating_add(1);
                    report(turn.events, || DataEvent::MsduRefused(reason));
                    turn.network.home(memory);
                }
            }
        }
        moved
    }

    fn publish<N, R: Rates<'id, P::Caps>, E, C>(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Option<PpduPlan> {
        let Self {
            data:
                Data::Flowing(Running {
                    form: StationRunning { plane, .. },
                    interface:
                        Interface::Idle {
                            gate: Gate::Open(gate),
                            ..
                        }
                        | Interface::Agreeing {
                            gate: Gate::Open(gate),
                            ..
                        },
                    ..
                }),
            entries:
                [
                    Entry::Linked(Linked {
                        carrying: Carrying::Carrying,
                        ..
                    }),
                ],
            ..
        } = self
        else {
            return None;
        };
        let rates = &mut *turn.rates;
        plane.publication(turn.port, gate, turn.now, |plan| rates.arena(0, plan))
    }

    fn next_queue(&self, _seal: Seal) -> Option<QueueId> {
        // A named TID's queue waits for its name's end.
        let hold = match &self.entries {
            [Entry::Linked(linked)] => linked.hold.tid().map(QueueId::of_tid),
            [Entry::Free] => None,
        };
        match &self.data {
            Data::Flowing(_) => self.queues.select(hold),
            _ => None,
        }
    }

    fn ended<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        _seal: Seal,
        facts: TxFacts,
        tbtt: Signal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) {
        let Self {
            data: Data::Flowing(running) | Data::Stopping(running),
            entries,
            queues,
            ..
        } = self
        else {
            // Stopped data has nothing of the port's.
            return;
        };
        let Running {
            form:
                StationRunning {
                    plane,
                    queues: rights,
                },
            singles,
            interface,
            ..
        } = running;
        // Nothing goes while a command that stops the intake runs.
        let gate = match interface {
            Interface::Idle {
                gate: Gate::Open(gate),
                ..
            }
            | Interface::Agreeing {
                gate: Gate::Open(gate),
                ..
            } => Some(&*gate),
            _ => None,
        };
        let rates = &mut *turn.rates;
        let network = &mut *turn.network;
        let settled = plane.completed(
            turn.port,
            facts,
            gate,
            turn.now,
            |plan| rates.arena(0, plan),
            |memory| network.home(memory),
            turn.events,
        );
        if let Some(settled) = settled {
            if let Some(lesson) = settled.lesson {
                turn.rates.learn(0, lesson, turn.now);
            }
            queues.charge(QueueId::of_tid(settled.tid), settled.cost);
            if let (Some((tid, head)), [Entry::Linked(linked)]) = (settled.bar, entries) {
                *tid.select_mut(&mut linked.owed) = Some(head);
            }
            queues.unblock();
        }
        let mut ends = StationEnds {
            singles,
            queues: rights,
            told: &mut *turn.told,
        };
        turn.port.serve(facts, tbtt, turn.now, &mut ends);
    }

    fn poll_command<N, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told<'id>>(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Option<Ending<'id, T>> {
        let (Data::Flowing(running) | Data::Stopping(running)) = &mut self.data else {
            return None;
        };
        let Running {
            form: StationRunning { queues, .. },
            singles,
            interface,
            ..
        } = running;
        let mut sink = StationEnds {
            singles,
            queues,
            told: &mut *turn.told,
        };
        Self::poll_interface(interface, turn.port, turn.now, &mut sink)
    }

    fn keyed<N: Network<P::Memory, P::Lent>, E: EventSink<DataEvent>>(
        &mut self,
        _seal: Seal,
        port: &mut P,
        network: &mut N,
        events: &mut E,
    ) {
        // What the arenas hold unpublished was written under the old key:
        // it goes home; the arena on the air settles at its end.
        let (Data::Flowing(running) | Data::Stopping(running)) = &mut self.data else {
            return;
        };
        let plane = &mut running.form.plane;
        let withdrawn = plane.withdraw_unpublished(port, |memory| network.home(memory), events);
        let [Entry::Linked(linked)] = &mut self.entries else {
            return;
        };
        for tid in WmmUserPriority::ALL {
            let went = tid.select(&withdrawn);
            // A number never transmitted named nothing to anyone: the
            // TID's counter goes back to the oldest of them, when they are
            // its last draws. A named TID's places hold nothing of it, so
            // nothing sets it back below its told number.
            if let Some(next) = went.rewind_to(linked.link.numbers.next(tid)) {
                linked.link.numbers.rewind(tid, next);
                plane.rewound(tid, next);
            }
            // A number that was transmitted was seen: a BlockAckReq to the
            // window's new head is owed.
            if went.transmitted
                && let Some(head) = plane.rehead(tid)
            {
                *tid.select_mut(&mut linked.owed) = Some(head);
            }
        }
    }

    fn unlink_form<N: Network<P::Memory, P::Lent>, E: EventSink<DataEvent>>(
        &mut self,
        _seal: Seal,
        _peer: u8,
        port: &mut P,
        network: &mut N,
        events: &mut E,
    ) {
        // The arena on the air settles at its end, its frames released.
        if let Data::Flowing(running) | Data::Stopping(running) = &mut self.data {
            running
                .form
                .plane
                .stop(port, |memory| network.home(memory), events);
        }
    }

    fn stopped(&mut self, _seal: Seal) -> Option<Self::Rights> {
        let Data::Stopping(Running {
            form: StationRunning { plane, .. },
            interface:
                Interface::Idle {
                    gate: Gate::Shut,
                    key: StationKey::Empty(_),
                    ..
                },
            ..
        }) = &self.data
        else {
            return None;
        };
        if !plane.is_idle() {
            return None;
        }
        let slots = agreement_slots(&mut self.data)?;
        // The running data moves out of its variant: a copy, once the
        // data's stop.
        let running = match core::mem::replace(&mut self.data, Data::Stopped) {
            Data::Stopping(running) => running,
            mut other => {
                // Checked stopping above: this does not happen. Should it,
                // the data keeps what it was, every right with it.
                if let Data::Flowing(running) | Data::Stopping(running) = &mut other {
                    running.agreements.slots_back(slots);
                }
                self.data = other;
                return None;
            }
        };
        let Running {
            form: StationRunning { mut plane, queues },
            singles,
            interface,
            groups,
            mut agreements,
            receive,
        } = running;
        match (interface, plane.idle_arenas()) {
            (
                Interface::Idle {
                    gate: Gate::Shut,
                    place: command,
                    key: StationKey::Empty(pairwise),
                },
                Some(arenas),
            ) => Some(DataRights {
                arenas,
                singles,
                data: [],
                queues,
                command,
                pairwise,
                groups,
                agreements: slots,
            }),
            (interface, None) => {
                agreements.slots_back(slots);
                // Checked idle above: this does not happen. Should it, the
                // data stays stopping with every part.
                self.data = Data::Stopping(Running {
                    form: StationRunning { plane, queues },
                    singles,
                    interface,
                    groups,
                    agreements,
                    receive,
                });
                None
            }
            (interface, Some(arenas)) => {
                agreements.slots_back(slots);
                // Checked shut, idle and keyless above: this does not
                // happen. Should it, the data stays stopping with every
                // right; its plane holds no link, so a new one with the
                // arenas is all it was.
                self.data = Data::Stopping(Running {
                    form: StationRunning {
                        plane: DataPlane::new(arenas),
                        queues,
                    },
                    singles,
                    interface,
                    groups,
                    agreements,
                    receive,
                });
                None
            }
        }
    }

    fn receive_round<N: Network<P::Memory, P::Lent>, R, E, C: Told<'id>>(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let (flow, running) = match &mut self.data {
            Data::Flowing(running) => (Flow::Flowing, running),
            Data::Stopping(running) => (Flow::Stopping, running),
            Data::Stopped => return Rest::Wake(None),
        };
        let Running {
            form: StationRunning { plane, .. },
            receive,
            agreements,
            ..
        } = running;
        // The access point's frames to the station or to a group, while it
        // is linked.
        let link = plane.current().map(|link| {
            (
                link.header.bssid(),
                link.header.source(),
                link.header.protection(),
                link.header.peer_qos(),
            )
        });
        let carrying = match &self.entries {
            [Entry::Linked(linked)] => Some(linked.carrying),
            [Entry::Free] => None,
        };
        // A QoS link admits receive agreements; without a link, or a BSSID
        // that is no individual address, the agreements end.
        let bssid = link.and_then(|(bssid, ..)| Bssid::new(IndividualAddress::new(bssid)?));
        let reorder = Reorder {
            agreements,
            local: link.map_or([0; 6], |(_, own, ..)| own),
            bssid,
            admitted: match link {
                Some((.., PeerQos::Qos)) => RxAgreements::Admitted,
                _ => RxAgreements::Declined,
            },
            link: carrying.and(bssid).map(|_| self.links),
        };
        receive.round(
            turn.port,
            turn.network,
            turn.told,
            turn.now,
            Links {
                flow,
                peer_of: |header: &RxHeader| {
                    let (bssid, own, protection, _) = link?;
                    let to_us =
                        header.receiver == own || IndividualAddress::new(header.receiver).is_none();
                    (header.transmitter == bssid && to_us).then_some(RxPeer {
                        peer: 0,
                        protection,
                        carrying: carrying?,
                    })
                },
                reorder: Reordering::Station(reorder),
            },
        )
    }

    fn own_number(&mut self, _seal: Seal, _peer: u8) -> Option<SequenceNumber> {
        match &mut self.entries {
            [Entry::Linked(linked)] => linked.link.numbers.take(LinkSpace::Station),
            [Entry::Free] => None,
        }
    }

    fn name_tid(&mut self, _seal: Seal, _peer: u8, tid: WmmUserPriority) -> Result<(), NoSpace> {
        match &mut self.entries {
            [Entry::Linked(linked)] if linked.link.numbers.qos() => {
                linked.hold = Hold::Draining(tid);
                Ok(())
            }
            _ => Err(NoSpace),
        }
    }

    fn tell_drained<C: Told<'id>>(&mut self, _seal: Seal, told: &mut C) {
        let (Data::Flowing(running), [Entry::Linked(linked)]) = (&self.data, &mut self.entries)
        else {
            return;
        };
        let Hold::Draining(tid) = linked.hold else {
            return;
        };
        if running.form.plane.oldest_held(tid).is_some() {
            return;
        }
        let number = linked.link.numbers.next(tid);
        linked.hold = Hold::Named(tid, number);
        told.tid_named(Peer::<1>(0), tid, number);
    }
}

/// The sink of an access point's port: its data's singles settle, each
/// giving its queue's right and data place back to its lane.
struct ApEnds<'a, 'id, K, R, E, C, M, const SINGLES: usize, const PEERS: usize, const POOL: usize> {
    lanes: &'a mut [Lane<'id, K>; 4],
    singles: &'a mut [SinglePlace<'id>; SINGLES],
    queues: &'a mut TxQueues<M, POOL, PEERS>,
    rates: &'a mut R,
    events: &'a mut E,
    told: &'a mut C,
    now: Instant,
}

impl<
    'id,
    K,
    A: Copy,
    D: Copy,
    R: Rates<'id, K>,
    E: EventSink<DataEvent>,
    C: Told<'id>,
    M,
    const SINGLES: usize,
    const PEERS: usize,
    const POOL: usize,
> PortSink<'id, A, D> for ApEnds<'_, 'id, K, R, E, C, M, SINGLES, PEERS, POOL>
{
    fn single_ended(
        &mut self,
        outcome: SingleOutcome<A, D>,
        place: SinglePlace<'id>,
        queue: QueueRight<'id>,
    ) {
        if usize::from(place.index()) < SINGLES {
            // A controller's single: the engine lends none, so this end
            // does not come; the place is written over its own index.
            if let Some(home) = self.singles.get_mut(usize::from(place.index())) {
                *home = place;
            }
            return;
        }
        // The port gives back what the engine lent it: the lane of the
        // queue is free again with both.
        let lane = queue.category().select_mut(self.lanes);
        let flight = match core::mem::replace(
            lane,
            Lane::Free {
                right: queue,
                place,
            },
        ) {
            Lane::Air(flight) => flight,
            free @ Lane::Free { .. } => {
                // A free lane holds its queue's one right, so the port
                // cannot give back another: this does not happen. Should it,
                // the lane keeps the right it held, and the end teaches
                // nothing.
                *lane = free;
                return;
            }
        };
        let delivered = matches!(outcome, TxOutcome::Ended(SingleEnd::Delivered { .. }));
        if !delivered {
            report(self.events, || {
                DataEvent::DataLost(crate::plane::single_loss(&outcome))
            });
        }
        self.queues.charge(
            flight.queue,
            single_cost(&flight.plan, flight.psdu, &outcome),
        );
        self.rates.learn(
            flight.peer,
            RateLesson::<A, (), D>::Single(outcome),
            self.now,
        );
        self.queues.unblock();
    }

    fn tbtt(&mut self, event: TbttEvent) {
        self.told.tbtt(event);
    }
}

/// An access point: its clients' data as singles.
impl<
    'id,
    P: ReceivePort<'id>,
    T: TxKey,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> Sections<'id, P> for access_point!()
where
    P::Ack: Copy,
    P::Denial: Copy,
{
    type Rights = DataRights<'id, SINGLES, GROUPS, AGREEMENTS, DATA_SINGLES>;
    type Key = T;

    fn start(&mut self, _seal: Seal, rights: Self::Rights) {
        let DataRights {
            arenas,
            singles,
            data:
                [
                    voice_place,
                    video_place,
                    best_effort_place,
                    background_place,
                ],
            queues: [voice, video, best_effort, background],
            command,
            pairwise,
            groups,
            agreements,
        } = rights;
        // QUEUES' order is voice, video, best effort, background; the lanes
        // are in the categories' selection order.
        self.data = Data::Flowing(Running {
            form: ApRunning {
                lanes: [
                    Lane::Free {
                        right: best_effort,
                        place: best_effort_place,
                    },
                    Lane::Free {
                        right: background,
                        place: background_place,
                    },
                    Lane::Free {
                        right: video,
                        place: video_place,
                    },
                    Lane::Free {
                        right: voice,
                        place: voice_place,
                    },
                ],
                arenas,
            },
            singles,
            // The access point's clients' keys are its controller's (step
            // 6): the station's slot stays empty.
            interface: Interface::Idle {
                gate: Gate::Shut,
                place: command,
                key: StationKey::Empty(pairwise),
            },
            groups,
            agreements: RxBlockAckStorage::new(agreements, self.policy.gap),
            receive: Receive::new(self.policy.late),
        });
    }

    fn route<N: Network<P::Memory, P::Lent>>(
        &self,
        _seal: Seal,
        network: &N,
        memory: &P::Memory,
    ) -> Result<QueueId, IntakeRefusal> {
        let Data::Flowing(_) = self.data else {
            return Err(IntakeRefusal::NoLink);
        };
        let frame = network.frame(memory, network.length(memory));
        let (header, _) = MsduHeader::parse(frame).map_err(|_| IntakeRefusal::ShortFrame)?;
        let found = self
            .entries
            .iter()
            .zip(0_u8..)
            .find_map(|(entry, peer)| match entry {
                Entry::Linked(linked) if linked.link.client == header.destination => {
                    Some((peer, linked.link.qos))
                }
                _ => None,
            });
        let Some((peer, qos)) = found else {
            return Err(IntakeRefusal::NoLink);
        };
        let tid = match qos {
            PeerQos::Qos => network.priority(memory),
            PeerQos::NonQos => WmmUserPriority::UP0,
        };
        QueueId::new(peer, tid).ok_or(IntakeRefusal::NoLink)
    }

    fn fill<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
        taken: &mut u8,
    ) -> u8 {
        let mut moved: u8 = 0;
        while *taken < self.budget {
            let Data::Flowing(Running {
                form: ApRunning { lanes, .. },
                interface:
                    Interface::Idle {
                        gate: Gate::Open(_),
                        ..
                    }
                    | Interface::Agreeing {
                        gate: Gate::Open(_),
                        ..
                    },
                ..
            }) = &self.data
            else {
                break;
            };
            let Some(queue) = self.queues.select(None) else {
                break;
            };
            let tid = queue.tid();
            let free = matches!(tid.access_category().select(lanes), Lane::Free { .. });
            let carries = matches!(
                self.entries.get(usize::from(queue.peer())),
                Some(Entry::Linked(Linked { carrying: Carrying::Carrying, hold, .. }))
                    if hold.tid() != Some(tid)
            );
            if !free || !carries {
                // This queue waits for its hardware queue, or for its link.
                self.queues.block(queue);
                continue;
            }
            let Some(msdu) = self.queues.pop(queue) else {
                break;
            };
            *taken = taken.saturating_add(1);
            moved = moved.saturating_add(1);
            self.send(queue, msdu, turn);
        }
        moved
    }

    fn publish<N, R: Rates<'id, P::Caps>, E, C>(
        &mut self,
        _seal: Seal,
        _turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Option<PpduPlan> {
        // A single goes as it is written: no arena is published.
        None
    }

    fn next_queue(&self, _seal: Seal) -> Option<QueueId> {
        let Data::Flowing(running) = &self.data else {
            return None;
        };
        self.queues.select(None).filter(|queue| {
            matches!(
                queue.tid().access_category().select(&running.form.lanes),
                Lane::Free { .. }
            )
        })
    }

    fn ended<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        _seal: Seal,
        facts: TxFacts,
        tbtt: Signal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) {
        let Self {
            data: Data::Flowing(running) | Data::Stopping(running),
            queues,
            ..
        } = self
        else {
            // Stopped data has nothing of the port's.
            return;
        };
        let mut ends = ApEnds {
            lanes: &mut running.form.lanes,
            singles: &mut running.singles,
            queues,
            rates: &mut *turn.rates,
            events: &mut *turn.events,
            told: &mut *turn.told,
            now: turn.now,
        };
        turn.port.serve(facts, tbtt, turn.now, &mut ends);
    }

    fn poll_command<N, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told<'id>>(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Option<Ending<'id, T>> {
        let Self {
            data: Data::Flowing(running) | Data::Stopping(running),
            queues,
            ..
        } = self
        else {
            return None;
        };
        let Running {
            form: ApRunning { lanes, .. },
            singles,
            interface,
            ..
        } = running;
        let mut ends = ApEnds {
            lanes,
            singles,
            queues,
            rates: &mut *turn.rates,
            events: &mut *turn.events,
            told: &mut *turn.told,
            now: turn.now,
        };
        Self::poll_interface(interface, turn.port, turn.now, &mut ends)
    }

    fn keyed<N: Network<P::Memory, P::Lent>, E: EventSink<DataEvent>>(
        &mut self,
        _seal: Seal,
        _port: &mut P,
        _network: &mut N,
        _events: &mut E,
    ) {
        // The access point's data asks no key's command in this half.
    }

    fn unlink_form<N: Network<P::Memory, P::Lent>, E: EventSink<DataEvent>>(
        &mut self,
        _seal: Seal,
        _peer: u8,
        _port: &mut P,
        _network: &mut N,
        _events: &mut E,
    ) {
        // A client's singles in flight hold no memory of the network: it
        // went home at the write.
    }

    fn stopped(&mut self, _seal: Seal) -> Option<Self::Rights> {
        let Data::Stopping(Running {
            form: ApRunning { lanes, .. },
            interface:
                Interface::Idle {
                    gate: Gate::Shut,
                    key: StationKey::Empty(_),
                    ..
                },
            ..
        }) = &self.data
        else {
            return None;
        };
        if lanes.iter().any(|lane| !matches!(lane, Lane::Free { .. })) {
            return None;
        }
        let slots = agreement_slots(&mut self.data)?;
        // The running data moves out of its variant: a copy, once the
        // data's stop.
        let running = match core::mem::replace(&mut self.data, Data::Stopped) {
            Data::Stopping(running) => running,
            mut other => {
                // Checked stopping above: this does not happen. Should it,
                // the data keeps what it was, every right with it.
                if let Data::Flowing(running) | Data::Stopping(running) = &mut other {
                    running.agreements.slots_back(slots);
                }
                self.data = other;
                return None;
            }
        };
        match running {
            Running {
                form:
                    ApRunning {
                        lanes:
                            [
                                Lane::Free {
                                    right: best_effort,
                                    place: best_effort_place,
                                },
                                Lane::Free {
                                    right: background,
                                    place: background_place,
                                },
                                Lane::Free {
                                    right: video,
                                    place: video_place,
                                },
                                Lane::Free {
                                    right: voice,
                                    place: voice_place,
                                },
                            ],
                        arenas,
                    },
                singles,
                interface:
                    Interface::Idle {
                        gate: Gate::Shut,
                        place: command,
                        key: StationKey::Empty(pairwise),
                    },
                groups,
                agreements: _,
                receive: _,
            } => Some(DataRights {
                arenas,
                singles,
                data: [
                    voice_place,
                    video_place,
                    best_effort_place,
                    background_place,
                ],
                queues: [voice, video, best_effort, background],
                command,
                pairwise,
                groups,
                agreements: slots,
            }),
            mut running => {
                // Checked above: this does not happen. Should it, the data
                // stays stopping with every part.
                running.agreements.slots_back(slots);
                self.data = Data::Stopping(running);
                None
            }
        }
    }

    fn receive_round<N: Network<P::Memory, P::Lent>, R, E, C: Told<'id>>(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let (flow, running) = match &mut self.data {
            Data::Flowing(running) => (Flow::Flowing, running),
            Data::Stopping(running) => (Flow::Stopping, running),
            Data::Stopped => return Rest::Wake(None),
        };
        let entries = &self.entries;
        // A client's frames to the access point.
        running.receive.round(
            turn.port,
            turn.network,
            turn.told,
            turn.now,
            Links {
                flow,
                peer_of: |header: &RxHeader| {
                    entries
                        .iter()
                        .zip(0_u8..)
                        .find_map(|(entry, peer)| match entry {
                            Entry::Linked(linked)
                                if header.transmitter == linked.link.client
                                    && header.receiver == linked.link.access_point =>
                            {
                                Some(RxPeer {
                                    peer,
                                    protection: LinkProtection::Open,
                                    carrying: linked.carrying,
                                })
                            }
                            _ => None,
                        })
                },
                reorder: Reordering::<'_, 'id, P::Caps, AGREEMENTS, WINDOW>::AccessPoint,
            },
        )
    }

    fn own_number(&mut self, _seal: Seal, peer: u8) -> Option<SequenceNumber> {
        match self.entries.get_mut(usize::from(peer)) {
            Some(Entry::Linked(linked)) => linked.link.numbers.take(LinkSpace::Station),
            _ => None,
        }
    }

    fn name_tid(&mut self, _seal: Seal, peer: u8, tid: WmmUserPriority) -> Result<(), NoSpace> {
        match self.entries.get_mut(usize::from(peer)) {
            Some(Entry::Linked(linked)) if linked.link.numbers.qos() => {
                linked.hold = Hold::Draining(tid);
                Ok(())
            }
            _ => Err(NoSpace),
        }
    }

    fn tell_drained<C: Told<'id>>(&mut self, _seal: Seal, told: &mut C) {
        let Data::Flowing(running) = &self.data else {
            return;
        };
        for (index, entry) in (0..=u8::MAX).zip(self.entries.iter_mut()) {
            let Entry::Linked(linked) = entry else {
                continue;
            };
            let Hold::Draining(tid) = linked.hold else {
                continue;
            };
            let queue = QueueId::new(index, tid);
            let held = running
                .form
                .lanes
                .iter()
                .any(|lane| matches!(lane, Lane::Air(flight) if Some(flight.queue) == queue));
            if held {
                continue;
            }
            let number = linked.link.numbers.next(tid);
            linked.hold = Hold::Named(tid, number);
            told.tid_named(Peer::<PEERS>(index), tid, number);
        }
    }
}
impl<
    'id,
    P: ReceivePort<'id>,
    T: TxKey,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> access_point!()
{
    /// Write `msdu` of `queue` by copy into its queue's data place and
    /// publish it; its memory goes home at the write. An MSDU that cannot
    /// go now (no rate) waits at its queue's head, the queue blocked until
    /// an end; one that can never go is refused. No number is drawn for an
    /// MSDU that does not go. The caller found the lane free, the gate open
    /// and the link carrying.
    fn send<
        N: Network<P::Memory, P::Lent>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        queue: QueueId,
        msdu: OutboundMsdu<P::Memory>,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) {
        let tid = queue.tid();
        let category = tid.access_category();
        let Self {
            data,
            entries,
            queues,
            ..
        } = self;
        let refuse = |turn: &mut Turn<'_, P, N, R, E, C>, memory, reason| {
            report(turn.events, || DataEvent::MsduRefused(reason));
            turn.network.home(memory);
        };
        let (
            Data::Flowing(Running {
                form: ApRunning { lanes, .. },
                interface:
                    Interface::Idle {
                        gate: Gate::Open(gate),
                        ..
                    }
                    | Interface::Agreeing {
                        gate: Gate::Open(gate),
                        ..
                    },
                ..
            }),
            Some(Entry::Linked(Linked { link, .. })),
        ) = (data, entries.get_mut(usize::from(queue.peer())))
        else {
            return refuse(turn, msdu.memory, IntakeRefusal::NoLink);
        };
        let Some(plan) = turn.rates.single(queue.peer(), tid) else {
            // It waits at its queue's head until an end unblocks it.
            if let Err(msdu) = queues.push_front(queue, msdu) {
                report(turn.events, || DataEvent::DataLost(DataLoss::Overflow));
                turn.network.home(msdu.memory);
            }
            queues.block(queue);
            return;
        };
        let frame = turn.network.frame(&msdu.memory, msdu.length);
        let Ok((msdu_header, payload)) = MsduHeader::parse(frame) else {
            return refuse(turn, msdu.memory, IntakeRefusal::ShortFrame);
        };
        let encapsulation = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            link.access_point,
            link.access_point,
            msdu_header,
            tid,
            link.qos,
            QosAckPolicy::Normal,
        );
        let header_len = usize::from(encapsulation.header_len);
        let length = header_len
            .checked_add(LLC_SNAP_HEADER_LEN)
            .and_then(|front| front.checked_add(payload.len()))
            .and_then(MpduLength::<P::Caps>::new);
        let Some(length) = length else {
            return refuse(turn, msdu.memory, IntakeRefusal::TooLong);
        };
        let counter = link.numbers.counter(tid);
        let header = MacHeader::decode(&encapsulation.header)
            .map(|mac| mac.with_sequence_control(counter.peek().sequence_control()));
        let described = header
            .and_then(|mac| HeaderSpec::from_mac_header(mac, IndividualAddress::new))
            .and_then(AckHeader::new)
            .and_then(|header| SizedFrame::new(header, length));
        let (Some(header), Some(described)) = (header, described) else {
            // The header the encapsulation wrote names an individual client
            // and holds in the MPDU: this does not happen.
            return refuse(turn, msdu.memory, IntakeRefusal::TooLong);
        };
        // The single's flight takes the lane; the lane's right and place go
        // to the port.
        let psdu =
            u32::try_from(length.get()).map_or(u32::MAX, |mpdu| mpdu.saturating_add(FCS_LEN));
        let lane = category.select_mut(lanes);
        let flight = Lane::Air(InFlight {
            peer: queue.peer(),
            queue,
            plan,
            psdu,
        });
        let (right, mut place) = match core::mem::replace(lane, flight) {
            Lane::Free { right, place } => (right, place),
            air @ Lane::Air(_) => {
                // The caller found the lane free: this does not happen.
                // Should it, the lane keeps its flight and the MSDU waits.
                *lane = air;
                if let Err(msdu) = queues.push_front(queue, msdu) {
                    report(turn.events, || DataEvent::DataLost(DataLoss::Overflow));
                    turn.network.home(msdu.memory);
                }
                queues.block(queue);
                return;
            }
        };
        let _number = counter.take();
        let llc_snap = encapsulation.llc_snap;
        turn.port.write_single(&mut place, |buffer| {
            let octets = header
                .octets()
                .chain(llc_snap)
                .chain(payload.iter().copied());
            for (out, octet) in buffer.iter_mut().zip(octets) {
                *out = octet;
            }
            length
        });
        // Copied out: the network's memory goes home at the write.
        turn.network.home(msdu.memory);
        let frame = TxFrame::Individual {
            frame: described,
            plan,
        };
        turn.port
            .publish_single(place, right, gate, &frame, turn.now);
    }
}

/// What a search reads of a form's running part and of a link.
#[cfg(test)]
pub trait Keyed {
    fn key(&self) -> std::string::String;
}

#[cfg(test)]
impl<K: oer_protocols_ieee80211_contracts_port::Ieee80211Capabilities> Keyed
    for StationRunning<'_, K>
{
    fn key(&self) -> std::string::String {
        self.plane.key()
    }
}

#[cfg(test)]
impl<K> Keyed for ApRunning<'_, K> {
    fn key(&self) -> std::string::String {
        self.lanes
            .iter()
            .map(|lane| match lane {
                Lane::Free { .. } => std::string::String::from("free;"),
                Lane::Air(flight) => {
                    std::format!("air{}|{:?}|{};", flight.peer, flight.queue, flight.psdu)
                }
            })
            .collect()
    }
}

#[cfg(test)]
impl<L: core::fmt::Debug> Keyed for StationEntry<L> {
    fn key(&self) -> std::string::String {
        std::format!("{:?}|{:?}", self.port, self.numbers.state_key())
    }
}

#[cfg(test)]
impl Keyed for ClientLink {
    fn key(&self) -> std::string::String {
        std::format!(
            "{:?}|{:?}|{:?}|{:?}",
            self.client,
            self.access_point,
            self.qos,
            self.numbers.state_key()
        )
    }
}

#[cfg(test)]
impl<
    'id,
    P: ReceivePort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const WINDOW: usize,
> engine!()
where
    F::Running<'id, P::Caps>: Keyed,
    F::Entry<P::Link, T>: Keyed,
    P::Memory: core::fmt::Debug,
{
    /// The engine's whole state, read from its private fields, for the
    /// search that merges equal states.
    pub(crate) fn key(&self) -> std::string::String {
        use core::fmt::Write;
        let running = |name: &str,
                       running: &Running<
            'id,
            F,
            P::Caps,
            T,
            SINGLES,
            GROUPS,
            AGREEMENTS,
            WINDOW,
        >| {
            let gate = |gate: &Gate<'id>| match gate {
                Gate::Shut => "shut",
                Gate::Open(_) => "open",
            };
            let key = |key: &StationKey<'id, T>| match key {
                StationKey::Empty(_) => "empty",
                StationKey::Installed(..) => "installed",
            };
            std::format!(
                "{name}|{}|{}|{}",
                running.receive.key(),
                match &running.interface {
                    Interface::Idle {
                        gate: g, key: k, ..
                    } => std::format!("idle {} {}", gate(g), key(k)),
                    Interface::Keying { gate: g, step, .. } => std::format!(
                        "keying {} {}",
                        gate(g),
                        match step {
                            KeyStep::Removing {
                                then: KeyTarget::Install(_),
                                ..
                            } => "removing, then installing",
                            KeyStep::Removing {
                                then: KeyTarget::Empty,
                                ..
                            } => "removing",
                            KeyStep::Installing { .. } => "installing",
                        }
                    ),
                    Interface::Closing { key: k, .. } => std::format!("closing {}", key(k)),
                    Interface::Agreeing {
                        gate: g,
                        key: k,
                        ticket,
                        ..
                    } => std::format!("agreeing {ticket:?} {} {}", gate(g), key(k)),
                    Interface::InTransition => std::string::String::from("moving"),
                },
                running.form.key()
            )
        };
        let mut key = std::format!(
            "{}|{}|",
            self.budget,
            match &self.data {
                Data::Stopped => std::string::String::from("stopped"),
                Data::Flowing(flowing) => running("flowing", flowing),
                Data::Stopping(stopping) => running("stopping", stopping),
            }
        );
        for entry in &self.entries {
            let _ = match entry {
                Entry::Free => write!(key, "free;"),
                Entry::Linked(linked) => write!(
                    key,
                    "{}|{:?}|{:?}|{:?};",
                    linked.link.key(),
                    linked.carrying,
                    linked.hold,
                    linked.owed
                ),
            };
        }
        let _ = write!(key, "{}", self.queues.key());
        key
    }
}

mod receive;
pub use receive::{AMSDU_MSDUS, CopyPermit, EthernetSpan, LendPermit, Room, RxRefusal, RxRefusals};
use receive::{Flow, Links, Receive, Reorder, Reordering, RxPeer};

#[cfg(test)]
mod tests;
