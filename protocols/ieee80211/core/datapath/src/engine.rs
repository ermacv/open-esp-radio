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
//!   [`LinkedPeer::name_next`], [`LinkedPeer::named`], and on the station's
//!   form `LinkedPeer::tx_agreement`, `LinkedPeer::tx_agreement_ended`;
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
use oer_portable_time::Instant;
pub use oer_protocols_ieee80211_contracts_port::DATA_SINGLES;
use oer_protocols_ieee80211_contracts_port::{
    AckHeader, AgreementSlot, Command, CommandEnd, CommandPlace, CommandPoll, DataPort, DataRights,
    GateClosed, GateOpen, GroupSlot, HeaderSpec, IdleArena, InCommand, MpduLength, PairwiseSlot,
    PortRate, PortSink, QueueRight, RetryPlan, Signal, SingleEnd, SingleOutcome, SinglePlace,
    SizedFrame, TbttEvent, TxFacts, TxFrame, TxOutcome,
};
use oer_protocols_ieee80211_core_installation::{DataStop, Rest};
use oer_protocols_ieee80211_wire::address::IndividualAddress;
use oer_protocols_ieee80211_wire::data::{
    DataInterfaceRole, LLC_SNAP_HEADER_LEN, MsduHeader, PeerQos, plan_data_encapsulation,
};
use oer_protocols_ieee80211_wire::frame::{MacHeader, QosAckPolicy};
use oer_protocols_ieee80211_wire::qos::{WmmAccessCategory, WmmUserPriority};
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

/// The network's end of the transmit path, which the binding implements
/// over the network's own interface. The network names no IEEE 802.11
/// type; the engine reads an MSDU's Ethernet frame in its memory. `M` is
/// the network's handle of an MSDU's memory.
pub trait Network<M> {
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
    type Entry<L, T> = StationEntry<L, T>;
    type Rights<'id, const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize> =
        DataRights<'id, SINGLES, GROUPS, AGREEMENTS, 0>;
}

impl Form for Singles {
    type Running<'id, K> = ApRunning<'id, K>;
    type Entry<L, T> = ClientLink<T>;
    type Rights<'id, const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize> =
        DataRights<'id, SINGLES, GROUPS, AGREEMENTS, DATA_SINGLES>;
}

/// A station's link to its access point, as its controller links it: the
/// plane's data state, the port's link its arenas open for, and the link's
/// transmit numbers.
pub struct StationLink<L, T> {
    pub data: DataLink,
    pub port: L,
    pub numbers: TxNumbers<T>,
}

/// What a station's linked entry keeps; the plane holds the link's data
/// state.
pub struct StationEntry<L, T> {
    port: L,
    numbers: TxNumbers<T>,
}

/// An access point's link to one client: its address, its own, whether it
/// negotiated QoS and the link's transmit numbers. Its data is open:
/// protection comes with the access point's keys.
pub struct ClientLink<T> {
    pub client: [u8; 6],
    pub access_point: [u8; 6],
    pub qos: PeerQos,
    pub numbers: TxNumbers<T>,
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

/// The gate the controller lent, with the interface's command place: the
/// gate's closing is this half's one command.
enum Gate<'id> {
    /// The controller holds the gate, closed.
    Shut(CommandPlace<'id>),
    /// Open: arenas and singles go.
    Open(GateOpen<'id>, CommandPlace<'id>),
    /// Its closing runs, with the command's place: nothing goes.
    Closing(InCommand<'id>),
    /// Inside one method, between two of the above: the port's command
    /// takes the open gate and the place by value, so safe Rust moves them
    /// out through a value left in their place. Never seen outside it.
    InTransition,
}

/// The data's rights and parts while it runs.
struct Running<'id, F: Form, K, const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize>
{
    form: F::Running<'id, K>,
    /// The controller's single places, which the engine does not lend.
    singles: [SinglePlace<'id>; SINGLES],
    gate: Gate<'id>,
    pairwise: PairwiseSlot<'id>,
    groups: [GroupSlot<'id>; GROUPS],
    agreements: [AgreementSlot<'id>; AGREEMENTS],
}

/// The data's state.
enum Data<'id, F: Form, K, const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize> {
    /// The installation machine holds the rights.
    Stopped,
    /// The data flows.
    Flowing(Running<'id, F, K, SINGLES, GROUPS, AGREEMENTS>),
    /// Asked to stop: every link ended; what is on the air or in the gate's
    /// closing ends first.
    Stopping(Running<'id, F, K, SINGLES, GROUPS, AGREEMENTS>),
}

/// A link's state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Carrying {
    /// Its data goes.
    Carrying,
    /// Its data waits: the controller holds the link.
    Held,
}

/// Whether a TID's data waits for the controller's frame that names the
/// number it was given.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Hold {
    Nothing,
    Named(WmmUserPriority),
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

/// The engine of one interface: `PEERS` peers, a pool of `POOL` MSDUs, and
/// the data's rights of a layout of `SINGLES` single places of its
/// controller, `GROUPS` group key slots and `AGREEMENTS` receive
/// agreements.
pub struct Engine<
    'id,
    P: DataPort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> {
    /// The MSDUs a section takes at most.
    budget: u8,
    data: Data<'id, F, P::Caps, SINGLES, GROUPS, AGREEMENTS>,
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
pub trait Sections<'id, P: DataPort<'id>> {
    /// The data's rights of the form's layout.
    type Rights;

    /// The data flows, with its rights.
    fn start(&mut self, _seal: Seal, rights: Self::Rights);

    /// The queue an MSDU goes into, or why it is refused.
    fn route<N: Network<P::Memory>>(
        &self,
        _seal: Seal,
        network: &N,
        memory: &P::Memory,
    ) -> Result<QueueId, IntakeRefusal>;

    /// Take queued MSDUs into the form's places, counting each taken
    /// against the budget in `taken`: the MSDUs that left their queues
    /// (taken, sent or refused).
    fn fill<N: Network<P::Memory>, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told<'id>>(
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
    fn ended<N: Network<P::Memory>, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told<'id>>(
        &mut self,
        _seal: Seal,
        facts: TxFacts,
        tbtt: Signal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    );

    /// Poll the gate's closing, through the form's sink.
    fn poll_command<N, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told<'id>>(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    );

    /// End the link of `peer`: what the form holds of it goes home.
    fn unlink_form<N: Network<P::Memory>, E: EventSink<DataEvent>>(
        &mut self,
        _seal: Seal,
        peer: u8,
        port: &mut P,
        network: &mut N,
        events: &mut E,
    );

    /// The form's transmissions on the air: an arena, or a single a queue.
    fn in_flight(&self, _seal: Seal) -> usize;

    /// The rights back and the data stopped, when it is stopping, its gate
    /// shut and nothing the port's; else nothing changes.
    fn stopped(&mut self, _seal: Seal) -> Option<Self::Rights>;

    /// The next number of `peer`'s link's `space`, taken.
    fn take_number(&mut self, _seal: Seal, peer: u8, space: LinkSpace) -> Option<SequenceNumber>;
}

/// The engine type of a handle's methods.
macro_rules! engine {
    () => {
        Engine<'id, P, F, T, PEERS, POOL, SINGLES, GROUPS, AGREEMENTS>
    };
}

/// A station's engine type.
macro_rules! station {
    () => {
        Engine<'id, P, Arenas, T, 1, POOL, SINGLES, GROUPS, AGREEMENTS>
    };
}

/// An access point's engine type.
macro_rules! access_point {
    () => {
        Engine<'id, P, Singles, T, PEERS, POOL, SINGLES, GROUPS, AGREEMENTS>
    };
}

impl<
    'id,
    P: DataPort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> engine!()
{
    /// The engine of stopped data, taking at most `budget` MSDUs a section
    /// (at least one): built in its place, a `const` item or a placed
    /// initializer.
    pub const fn new(budget: u8) -> Self {
        Self {
            budget: if budget == 0 { 1 } else { budget },
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

    /// The data's gate, while it runs.
    const fn gate_mut(&mut self) -> Option<&mut Gate<'id>> {
        match &mut self.data {
            Data::Flowing(running) | Data::Stopping(running) => Some(&mut running.gate),
            Data::Stopped => None,
        }
    }

    /// The MSDUs the queues hold.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.queues.len()
    }

    /// Begin the gate's closing, when it is open: nothing new goes from now.
    fn close_gate<N, R, E, C>(&mut self, turn: &mut Turn<'_, P, N, R, E, C>) {
        let Some(gate) = self.gate_mut() else {
            return;
        };
        let Gate::Open(..) = gate else {
            return;
        };
        if let Gate::Open(open, place) = core::mem::replace(gate, Gate::InTransition) {
            *gate = Gate::Closing(turn.port.begin(place, Command::CloseGate(open), turn.now));
        }
    }

    /// The gate's command polled through `sink`: its end puts the
    /// command's place back and gives the closed gate, for the controller.
    fn poll_through(
        gate: &mut Gate<'id>,
        port: &mut P,
        now: Instant,
        sink: &mut impl PortSink<'id, P::Ack, P::Denial>,
    ) -> Option<GateClosed<'id>> {
        let Gate::Closing(_) = gate else {
            return None;
        };
        let Gate::Closing(command) = core::mem::replace(gate, Gate::InTransition) else {
            return None;
        };
        match port.poll_command(command, now, sink) {
            CommandPoll::Pending(command) => {
                *gate = Gate::Closing(command);
                None
            }
            CommandPoll::Ended(place, end) => {
                *gate = Gate::Shut(place);
                match end {
                    CommandEnd::GateClosed(closed) => Some(closed),
                    _ => None,
                }
            }
        }
    }
}

impl<
    'id,
    P: DataPort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> engine!()
where
    Self: Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// The network has MSDUs.
    pub fn offered<
        N: Network<P::Memory>,
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
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        self.poll_command(Seal(()), turn);
        self.intake(turn)
    }

    /// The port's transmit facts: the arena's end and the singles', then
    /// the command in progress, then the intake.
    pub fn transmitted<
        N: Network<P::Memory>,
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
        self.poll_command(Seal(()), turn);
        self.intake(turn)
    }

    /// The rights back, when the data is stopping, the gate shut and
    /// nothing the port's.
    fn finish(&mut self) -> DataStop<F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>> {
        if self.in_flight(Seal(())) != 0 {
            return DataStop::Going;
        }
        match self.stopped(Seal(())) {
            Some(rights) => DataStop::Stopped(rights),
            None => DataStop::Going,
        }
    }

    /// End `peer`'s link: its queued MSDUs go home, its entry is free.
    fn unlink<N: Network<P::Memory>, E: EventSink<DataEvent>>(
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
        self.queues.reset_peer(peer);
        self.unlink_form(Seal(()), peer, port, network, events);
        if let Some(entry) = self.entries.get_mut(usize::from(peer)) {
            *entry = Entry::Free;
        }
    }

    /// Link the free `peer`: its entry holds `link`.
    fn link<
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        peer: u8,
        link: F::Entry<P::Link, T>,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        if let Some(entry) = self.entries.get_mut(usize::from(peer)) {
            *entry = Entry::Linked(Linked {
                link,
                carrying: Carrying::Carrying,
                hold: Hold::Nothing,
                owed: [None; 8],
            });
        }
        self.queues.reset_peer(peer);
        self.intake(turn)
    }

    /// What the network offers, into the queues while the pool has room;
    /// what has no link goes back at once. Whether it moved anything.
    fn drain<N: Network<P::Memory>, R, E: EventSink<DataEvent>, C>(
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

    /// The intake: drain, fill, publish, again while it moved something,
    /// within one budget. A round is owed when the budget ran out with more
    /// to take.
    fn intake<
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
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

/// What the gate's place is now.
pub enum GatePlace<'a, E> {
    /// The controller holds it, closed.
    Shut(ShutGate<'a, E>),
    /// The engine holds it open.
    Open(OpenGate<'a, E>),
    /// Its closing runs in the port; it ends in [`Told::gate_closed`].
    Closing(ClosingGate<'a, E>),
}

/// A shut gate's handle.
pub struct ShutGate<'a, E>(&'a mut E);

/// An open gate's handle.
pub struct OpenGate<'a, E>(&'a mut E);

/// A closing gate's handle: it offers no transition.
pub struct ClosingGate<'a, E>(PhantomData<&'a mut E>);

impl<
    'a,
    'id,
    P: DataPort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> FlowingData<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>>,
    P::Ack: Copy,
    P::Denial: Copy,
{
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
        let open = matches!(engine.gate_mut(), Some(Gate::Open(..)));
        let shut = matches!(engine.gate_mut(), Some(Gate::Shut(_)) | None);
        match (open, shut) {
            (true, _) => GatePlace::Open(OpenGate(engine)),
            (false, true) => GatePlace::Shut(ShutGate(engine)),
            (false, false) => GatePlace::Closing(ClosingGate(PhantomData)),
        }
    }

    /// Stop the data, as the installation machine asks: every link ends,
    /// its queued and unpublished data goes home, an open gate closes. The
    /// rights come back once nothing is the port's; until
    /// then the answer is [`DataStop::Going`], and the machine asks again
    /// ([`StoppingData::poll`]).
    pub fn stop<
        N: Network<P::Memory>,
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
        engine.close_gate(turn);
        // With no link, what the network still offers goes back.
        engine.drain(turn);
        engine.finish()
    }
}

impl<
    'a,
    'id,
    P: DataPort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> StoppingData<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// The machine asks the data's stop again: the gate's closing is
    /// polled, and the rights come back once nothing is the port's.
    pub fn poll<
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> DataStop<F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>> {
        let engine = self.0;
        engine.poll_command(Seal(()), turn);
        engine.finish()
    }
}

impl<
    'a,
    'id,
    P: DataPort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> LinkedPeer<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// End the link: its queued data goes home.
    pub fn unlink<
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let Self { engine, peer } = self;
        engine.unlink(peer, turn.port, turn.network, turn.events);
        engine.intake(turn)
    }

    /// Change the linked entry and run the intake. The handle borrows the
    /// engine, so the peer is linked while the handle lives.
    fn change<
        N: Network<P::Memory>,
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
        N: Network<P::Memory>,
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
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        self.change(turn, |linked| linked.carrying = Carrying::Carrying)
    }

    /// The controller's frame that names the held number went: the held
    /// TID's data goes on.
    pub fn named<
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        self.change(turn, |linked| linked.hold = Hold::Nothing)
    }

    /// The next number of the link's `space`, for a frame of the
    /// controller: a TID's data waits until the controller says the frame
    /// that names it went ([`Self::named`]).
    ///
    /// # Errors
    ///
    /// The link has no such space: its peer did not negotiate QoS.
    pub fn name_next(&mut self, space: LinkSpace) -> Result<SequenceNumber, NoSpace> {
        let number = self
            .engine
            .take_number(Seal(()), self.peer, space)
            .ok_or(NoSpace)?;
        if let (LinkSpace::Tid(tid), Some(linked)) = (space, self.engine.linked_mut(self.peer)) {
            linked.hold = Hold::Named(tid);
        }
        Ok(number)
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
    P: DataPort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> ShutGate<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// Open the closed gate the controller lends: arenas and singles go.
    pub fn open<
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        gate: GateClosed<'id>,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let engine = self.0;
        if let Some(place) = engine.gate_mut() {
            *place = match core::mem::replace(place, Gate::InTransition) {
                Gate::Shut(command) => Gate::Open(turn.port.open_gate(gate), command),
                // The handle is a shut gate's: the data holds no other.
                other => other,
            };
        }
        engine.intake(turn)
    }
}

impl<
    'a,
    'id,
    P: DataPort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> OpenGate<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// Close the gate: nothing new goes, and the port closes it once its
    /// air is quiet; the closed gate then goes to the controller
    /// ([`Told::gate_closed`]).
    pub fn close<
        N: Network<P::Memory>,
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
    P: DataPort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> StoppedData<'a, engine!()>
where
    engine!(): Sections<'id, P, Rights = F::Rights<'id, SINGLES, GROUPS, AGREEMENTS>>,
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
    P: DataPort<'id>,
    T: TxKey,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
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
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        link: StationLink<P::Link, T>,
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
    P: DataPort<'id>,
    T: TxKey,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> FreePeer<'a, access_point!()>
where
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// Link the client.
    pub fn link<
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        self,
        link: ClientLink<T>,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let Self { engine, peer } = self;
        engine.link(peer, link, turn)
    }
}

/// The station's transmit agreements: the arenas' form alone aggregates.
impl<
    'a,
    'id,
    P: DataPort<'id>,
    T: TxKey,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> LinkedPeer<'a, station!()>
where
    P::Ack: Copy,
    P::BlockAck: Copy,
    P::Denial: Copy,
{
    /// The TID's transmit agreement is operational with a window of `size`,
    /// 1 to 64; its head is derived: the oldest number the link's arenas
    /// hold of the TID, else its next.
    pub fn tx_agreement<
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        tid: WmmUserPriority,
        size: u16,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let engine = &mut *self.engine;
        if let (Data::Flowing(running), [Entry::Linked(linked)]) =
            (&mut engine.data, &engine.entries)
        {
            let plane = &mut running.form.plane;
            let window = TxWindow {
                head: match plane.oldest_held(tid) {
                    Some(oldest) => oldest,
                    None => linked.link.numbers.next(tid),
                },
                size,
            };
            plane.agreement(tid, Some(window));
        }
        engine.queues.unblock();
        engine.intake(turn)
    }

    /// The TID's transmit agreement ended.
    pub fn tx_agreement_ended<
        N: Network<P::Memory>,
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
    P: DataPort<'id>,
    T,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
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
    P: DataPort<'id>,
    T: TxKey,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> Sections<'id, P> for station!()
where
    P::Ack: Copy,
    P::BlockAck: Copy,
    P::Denial: Copy,
{
    type Rights = DataRights<'id, SINGLES, GROUPS, AGREEMENTS, 0>;

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
            gate: Gate::Shut(command),
            pairwise,
            groups,
            agreements,
        });
    }

    fn route<N: Network<P::Memory>>(
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
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told<'id>,
    >(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
        taken: &mut u8,
    ) -> u8 {
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
        } = self
        else {
            return 0;
        };
        let plane = &mut running.form.plane;
        let hold = match hold {
            Hold::Named(tid) => Some(QueueId::of_tid(*tid)),
            Hold::Nothing => None,
        };
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
            match plane.intake(turn.port, &link.port, msdu, &mut link.numbers, turn.now) {
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
                    gate: Gate::Open(gate, _),
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
        match &self.data {
            Data::Flowing(running) if running.form.plane.takes() => self.queues.select(None),
            _ => None,
        }
    }

    fn ended<
        N: Network<P::Memory>,
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
            gate,
            ..
        } = running;
        let gate = match gate {
            Gate::Open(gate, _) => Some(&*gate),
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
    ) {
        let (Data::Flowing(running) | Data::Stopping(running)) = &mut self.data else {
            return;
        };
        let Running {
            form: StationRunning { queues, .. },
            singles,
            gate,
            ..
        } = running;
        let Gate::Closing(_) = gate else {
            return;
        };
        let mut sink = StationEnds {
            singles,
            queues,
            told: &mut *turn.told,
        };
        let closed = Self::poll_through(gate, turn.port, turn.now, &mut sink);
        if let Some(closed) = closed {
            turn.told.gate_closed(closed);
        }
    }

    fn unlink_form<N: Network<P::Memory>, E: EventSink<DataEvent>>(
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

    fn in_flight(&self, _seal: Seal) -> usize {
        match &self.data {
            Data::Flowing(running) | Data::Stopping(running) => {
                usize::from(running.form.plane.on_air())
            }
            Data::Stopped => 0,
        }
    }

    fn stopped(&mut self, _seal: Seal) -> Option<Self::Rights> {
        let Data::Stopping(Running {
            form: StationRunning { plane, .. },
            gate: Gate::Shut(_),
            ..
        }) = &self.data
        else {
            return None;
        };
        if !plane.is_idle() {
            return None;
        }
        // The running data moves out of its variant: a copy, once the
        // data's stop.
        let Data::Stopping(running) = core::mem::replace(&mut self.data, Data::Stopped) else {
            return None;
        };
        let Running {
            form: StationRunning { mut plane, queues },
            singles,
            gate,
            pairwise,
            groups,
            agreements,
        } = running;
        match (gate, plane.idle_arenas()) {
            (Gate::Shut(command), Some(arenas)) => Some(DataRights {
                arenas,
                singles,
                data: [],
                queues,
                command,
                pairwise,
                groups,
                agreements,
            }),
            (gate, None) => {
                // Checked idle above: this does not happen. Should it, the
                // data stays stopping with every part.
                self.data = Data::Stopping(Running {
                    form: StationRunning { plane, queues },
                    singles,
                    gate,
                    pairwise,
                    groups,
                    agreements,
                });
                None
            }
            (gate, Some(arenas)) => {
                // Checked shut above: this does not happen. Should it, the
                // data stays stopping with every right; its plane holds no
                // link, so a new one with the arenas is all it was.
                self.data = Data::Stopping(Running {
                    form: StationRunning {
                        plane: DataPlane::new(arenas),
                        queues,
                    },
                    singles,
                    gate,
                    pairwise,
                    groups,
                    agreements,
                });
                None
            }
        }
    }

    fn take_number(&mut self, _seal: Seal, _peer: u8, space: LinkSpace) -> Option<SequenceNumber> {
        match &mut self.entries {
            [Entry::Linked(linked)] => linked.link.numbers.take(space),
            [Entry::Free] => None,
        }
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
        let Lane::Air(flight) = core::mem::replace(
            lane,
            Lane::Free {
                right: queue,
                place,
            },
        ) else {
            return;
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
    P: DataPort<'id>,
    T: TxKey,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> Sections<'id, P> for access_point!()
where
    P::Ack: Copy,
    P::Denial: Copy,
{
    type Rights = DataRights<'id, SINGLES, GROUPS, AGREEMENTS, DATA_SINGLES>;

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
            gate: Gate::Shut(command),
            pairwise,
            groups,
            agreements,
        });
    }

    fn route<N: Network<P::Memory>>(
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
        N: Network<P::Memory>,
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
                gate: Gate::Open(..),
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
                    if *hold != Hold::Named(tid)
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
        N: Network<P::Memory>,
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
    ) {
        let Self {
            data: Data::Flowing(running) | Data::Stopping(running),
            queues,
            ..
        } = self
        else {
            return;
        };
        let Running {
            form: ApRunning { lanes, .. },
            singles,
            gate,
            ..
        } = running;
        let Gate::Closing(_) = gate else {
            return;
        };
        let mut ends = ApEnds {
            lanes,
            singles,
            queues,
            rates: &mut *turn.rates,
            events: &mut *turn.events,
            told: &mut *turn.told,
            now: turn.now,
        };
        let closed = Self::poll_through(gate, turn.port, turn.now, &mut ends);
        if let Some(closed) = closed {
            turn.told.gate_closed(closed);
        }
    }

    fn unlink_form<N: Network<P::Memory>, E: EventSink<DataEvent>>(
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

    fn in_flight(&self, _seal: Seal) -> usize {
        match &self.data {
            Data::Flowing(running) | Data::Stopping(running) => running
                .form
                .lanes
                .iter()
                .filter(|lane| matches!(lane, Lane::Air(_)))
                .count(),
            Data::Stopped => 0,
        }
    }

    fn stopped(&mut self, _seal: Seal) -> Option<Self::Rights> {
        let Data::Stopping(Running {
            form: ApRunning { lanes, .. },
            gate: Gate::Shut(_),
            ..
        }) = &self.data
        else {
            return None;
        };
        if lanes.iter().any(|lane| !matches!(lane, Lane::Free { .. })) {
            return None;
        }
        // The running data moves out of its variant: a copy, once the
        // data's stop.
        let Data::Stopping(running) = core::mem::replace(&mut self.data, Data::Stopped) else {
            return None;
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
                gate: Gate::Shut(command),
                pairwise,
                groups,
                agreements,
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
                agreements,
            }),
            running => {
                // Checked above: this does not happen. Should it, the data
                // stays stopping with every part.
                self.data = Data::Stopping(running);
                None
            }
        }
    }

    fn take_number(&mut self, _seal: Seal, peer: u8, space: LinkSpace) -> Option<SequenceNumber> {
        match self.entries.get_mut(usize::from(peer)) {
            Some(Entry::Linked(linked)) => linked.link.numbers.take(space),
            _ => None,
        }
    }
}
impl<
    'id,
    P: DataPort<'id>,
    T: TxKey,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> access_point!()
{
    /// Write `msdu` of `queue` by copy into its queue's data place and
    /// publish it; its memory goes home at the write. An MSDU that cannot
    /// go now (no rate) waits at its queue's head, the queue blocked until
    /// an end; one that can never go is refused. No number is drawn for an
    /// MSDU that does not go. The caller found the lane free, the gate open
    /// and the link carrying.
    fn send<
        N: Network<P::Memory>,
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
                gate: Gate::Open(gate, _),
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
        let (counter, _key) = link.numbers.data_and_key(tid);
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
impl<L: core::fmt::Debug, T> Keyed for StationEntry<L, T> {
    fn key(&self) -> std::string::String {
        std::format!("{:?}|{:?}", self.port, self.numbers.state_key())
    }
}

#[cfg(test)]
impl<T> Keyed for ClientLink<T> {
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
    P: DataPort<'id>,
    F: Form,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
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
        let running =
            |name: &str, running: &Running<'id, F, P::Caps, SINGLES, GROUPS, AGREEMENTS>| {
                std::format!(
                    "{name}|{}|{}",
                    match running.gate {
                        Gate::Shut(_) => "shut",
                        Gate::Open(..) => "open",
                        Gate::Closing(_) => "closing",
                        Gate::InTransition => "moving",
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

#[cfg(test)]
mod tests;
