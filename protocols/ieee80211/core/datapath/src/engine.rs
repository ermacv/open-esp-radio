//! The engine's transmit half (`design/engine-contract-2026-11-15.md`,
//! B3e, under `design/port-contract-2026-11-16.md` where the two differ):
//! one sans-IO core an interface, between the port, the network's
//! transport and the interface's controller.
//!
//! The engine holds the data's rights while the data flows
//! ([`Engine::flows`] to [`Engine::stop_data`]), the peer table (a free
//! entry or a link a peer), the transmit queues a peer and TID
//! ([`TxQueues`]) and the gate the controller lent it. Its sections are
//! functions of what woke the pump, each lent the port, the network's end,
//! the rate policy and the instant ([`Turn`]):
//!
//! - [`Engine::offered`]: the network has MSDUs. The intake is one loop of
//!   drain, fill and publish, repeated while it changed something, within
//!   one budget of MSDUs taken; a budget that ran out with more to take
//!   owes a round ([`Rest::Round`]).
//! - [`Engine::transmitted`]: the port's transmit facts. An arena's end is
//!   read, the other arena published at once when the end allows it, the
//!   arena settled; then the singles' ends; then the command in progress;
//!   then the intake again.
//! - [`Engine::asked`], [`Engine::link`], [`Engine::name_next`]: the
//!   controller's requests.
//! - [`Engine::stop_data`]: the installation machine's "stop the data".
//!
//! An interface's data goes in one of two forms ([`Form`], D1 of the
//! engine's contract): a station's in the port's two arenas
//! ([`Arenas`], the [`DataPlane`]), an access point's as single MPDUs
//! ([`Singles`]) until its aggregation. A single is written into a single's
//! place by copy (`DataPort::write_single`): the MAC's DMA does not read the
//! network's memory on the ESP32-S31, so the memory goes home at the write.
//! An access point's data has one single's place a hardware queue, the last
//! [`DATA_SINGLES`] of its layout's places, one a queue in
//! [`QUEUES`](oer_protocols_ieee80211_contracts_port::QUEUES)' order; a
//! station's layout has none.

use core::marker::PhantomData;

use oer_portable_events::{EventSink, report};
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    AckHeader, AgreementSlot, Command, CommandEnd, CommandPlace, CommandPoll, DataPort, DataRights,
    GateClosed, GateOpen, GroupSlot, HeaderSpec, IdleArena, InCommand, MpduLength, PairwiseSlot,
    PortRate, PortSink, QUEUES, QueueRight, RetryPlan, Signal, SingleEnd, SingleOutcome,
    SinglePlace, SizedFrame, TbttEvent, TxFacts, TxFrame, TxOutcome,
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

/// The single's places an access point's data takes: one a hardware queue,
/// the last of its layout's places.
pub const DATA_SINGLES: usize = QUEUES.len();

/// The single's places of an access point's layout with `MANAGEMENT`
/// places of its controller's: its data's [`DATA_SINGLES`] after them.
pub const fn access_point_singles<const MANAGEMENT: usize>() -> usize {
    const {
        match MANAGEMENT.checked_add(DATA_SINGLES) {
            Some(singles) => singles,
            None => panic!("an access point's places are counted"),
        }
    }
}

/// The FCS every MPDU carries on the air (IEEE Std 802.11-2020, 9.2.4.8).
const FCS_LEN: u32 = 4;

/// The network's end of the transmit path, which the binding implements
/// over the network's own interface. The network names no IEEE 802.11
/// type; the engine reads an MSDU's Ethernet frame in its memory.
pub trait Network<M> {
    /// The next MSDU the network offers; `None` while it offers none.
    fn take(&mut self) -> Option<OutboundMsdu<M>>;

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
pub trait Told {
    /// The port served a TBTT of the station's schedule.
    fn tbtt(&mut self, event: TbttEvent);
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

mod sealed {
    pub trait Sealed {}
}

/// How an interface's data goes: a closed set.
pub trait Form: sealed::Sealed {
    /// What the form keeps of the data path: a station's plane, an access
    /// point's arenas aside.
    type Sender<'id, K>;
    /// What a link request carries for the form.
    type Link<L, T>;
    /// What a linked peer's entry keeps.
    type Entry<L, T>;
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
    type Sender<'id, K> = DataPlane<'id, K>;
    type Link<L, T> = StationLink<L, T>;
    type Entry<L, T> = StationEntry<L, T>;
}

impl Form for Singles {
    type Sender<'id, K> = Aside<'id>;
    type Link<L, T> = ClientLink<T>;
    type Entry<L, T> = ClientLink<T>;
}

/// A station's link: the data plane's state, the port's link its arenas
/// open for, and its transmit numbers.
pub struct StationLink<L, T> {
    pub data: DataLink,
    pub port: L,
    pub numbers: TxNumbers<T>,
}

/// What a station's linked entry keeps; the plane holds its data state.
pub struct StationEntry<L, T> {
    port: L,
    numbers: TxNumbers<T>,
}

/// An access point's link to one client: its address, its own, whether it
/// negotiated QoS and the link's transmit numbers. Its data is open:
/// protection comes with the access point's keys.
pub struct ClientLink<T> {
    pub client: IndividualAddress,
    pub access_point: IndividualAddress,
    pub qos: PeerQos,
    pub numbers: TxNumbers<T>,
}

/// An access point's arenas, which its data does not use, held while the
/// data flows.
pub enum Aside<'id> {
    /// With the installation.
    Away,
    /// Here.
    Here([IdleArena<'id>; 2]),
}

/// One right's place while the data flows.
enum Slot<T> {
    /// With the installation: the data is stopped.
    Away,
    /// Here.
    Here(T),
    /// The port's, until the end that gives it back.
    Port,
}

impl<T> Slot<T> {
    /// Where the right is, for the search's key.
    #[cfg(test)]
    const fn mark(&self) -> char {
        match self {
            Self::Away => 'a',
            Self::Here(_) => 'h',
            Self::Port => 'p',
        }
    }

    const fn is_here(&self) -> bool {
        matches!(self, Self::Here(_))
    }

    /// The right, the slot left to the port.
    fn lend(&mut self) -> Option<T> {
        match core::mem::replace(self, Self::Port) {
            Self::Here(right) => Some(right),
            other => {
                *self = other;
                None
            }
        }
    }
}

/// Every slot's right, when all are here; else nothing moves.
fn gather<T, const N: usize>(slots: &mut [Slot<T>; N]) -> Option<[T; N]> {
    if !slots.iter().all(Slot::is_here) {
        return None;
    }
    let mut rights = heapless::Vec::<T, N>::new();
    for slot in slots.iter_mut() {
        if let Slot::Here(right) = core::mem::replace(slot, Slot::Away) {
            // As many as the slots: the vector holds them.
            let _kept: Result<(), T> = rights.push(right);
        }
    }
    rights.into_array().ok()
}

/// A hardware queue's state for the data.
enum Lane<'id, K> {
    /// With the installation.
    Away,
    /// Free: its right.
    Free(QueueRight<'id>),
    /// A single of the access point's data on it: the port holds its right
    /// and its place until its end.
    Air(InFlight<'id, K>),
}

/// What the engine keeps of a single in flight, for its end.
struct InFlight<'id, K> {
    peer: u8,
    queue: QueueId,
    plan: RetryPlan<PortRate<'id, K>>,
    psdu: u32,
}

/// A link's state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Carrying {
    /// Its data goes.
    Carrying,
    /// Its data waits: the controller holds the link.
    Held,
}

/// One entry of the peer table.
enum Entry<E> {
    Free,
    Linked {
        link: E,
        carrying: Carrying,
        /// The TID whose data waits for the controller's frame that names
        /// the number it was given.
        hold: Option<WmmUserPriority>,
    },
}

/// The gate the controller lent.
enum Gate<'id> {
    /// The controller holds it, closed.
    Away,
    /// Open: arenas and singles go.
    Open(GateOpen<'id>),
    /// Its closing is asked: nothing goes.
    Closing,
    /// Closed, for the controller to take.
    Closed(GateClosed<'id>),
}

/// The interface's command place.
enum Commanding<'id> {
    Away,
    Here(CommandPlace<'id>),
    /// The gate's closing, in progress.
    CloseGate(InCommand<'id>),
}

/// Whether the data flows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Flow {
    Stopped,
    Flowing,
    /// Asked to stop; what is on the air or in a command ends first.
    Stopping,
}

/// The engine's data is not flowing: it holds no rights to give back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotFlowing;

/// Why a request was refused; nothing changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refused {
    /// The data does not flow.
    NotFlowing,
    /// The peer has a link already.
    Linked,
    /// The peer has no link.
    NotLinked,
    /// The form has no transmit agreements: an access point's data goes as
    /// single MPDUs.
    NoAgreements,
    /// The plane holds a link already.
    PlaneLinked,
    /// The engine holds a gate already.
    GateHeld,
}

/// What a refused request hands back, with why.
pub type Refusal<T> = (T, Refused);

/// A link taken into a form: its entry, or the link back with why.
pub type Linked<F, L, T> = Result<<F as Form>::Entry<L, T>, Refusal<<F as Form>::Link<L, T>>>;

/// A request of the controller.
pub enum Request<const PEERS: usize> {
    /// End the peer's link: its queued data goes home.
    Unlink(Peer<PEERS>),
    /// Hold the peer's data.
    Hold(Peer<PEERS>),
    /// Let it go again.
    Release(Peer<PEERS>),
    /// The TID's transmit agreement is operational with a window of this
    /// size, 1 to 64; its head is derived: the oldest number the link's
    /// arenas hold of the TID, else its next.
    TxAgreement(Peer<PEERS>, WmmUserPriority, u16),
    /// The TID's transmit agreement ended.
    TxAgreementEnded(Peer<PEERS>, WmmUserPriority),
    /// The controller's frame that names the held number went: the held
    /// TID's data goes on.
    Named(Peer<PEERS>),
    /// Close the gate: nothing new goes, and the port closes it once its
    /// air is quiet.
    CloseGate,
}

/// What the engine owes the controller of one link.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Owed {
    /// A BlockAckReq a TID, to the window's head.
    bars: [Option<SequenceNumber>; 8],
}

/// The engine of one interface: `PEERS` peers, a pool of `POOL` MSDUs, and
/// the data's rights of a layout of `SINGLES` single's places, `GROUPS`
/// group key slots and `AGREEMENTS` receive agreements.
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
    flow: Flow,
    /// The MSDUs a section takes at most.
    budget: u8,
    peers: [Entry<F::Entry<P::Link, T>>; PEERS],
    owed: [Owed; PEERS],
    queues: TxQueues<P::Memory, POOL, PEERS>,
    sender: F::Sender<'id, P::Caps>,
    /// The hardware queues, in [`WmmAccessCategory::select`]'s order.
    lanes: [Lane<'id, P::Caps>; 4],
    singles: [Slot<SinglePlace<'id>>; SINGLES],
    command: Commanding<'id>,
    pairwise: Slot<PairwiseSlot<'id>>,
    groups: [Slot<GroupSlot<'id>>; GROUPS],
    agreements: [Slot<AgreementSlot<'id>>; AGREEMENTS],
    gate: Gate<'id>,
    brand: PhantomData<fn(&'id ()) -> &'id ()>,
}

/// The rights of the engine's layout.
pub type Rights<'id, const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize> =
    DataRights<'id, SINGLES, GROUPS, AGREEMENTS>;

/// The form-specific half of the sections, which only the engine calls
/// ([`Seal`]).
pub trait Sections<'id, P: DataPort<'id>, T, const PEERS: usize> {
    /// The queue an MSDU goes into, or why it is refused.
    fn route<N: Network<P::Memory>>(
        &self,
        _seal: Seal,
        network: &N,
        msdu: &OutboundMsdu<P::Memory>,
    ) -> Result<QueueId, IntakeRefusal>;

    /// Take queued MSDUs into the form's places, counting each taken
    /// against the budget in `taken`; the MSDUs that left their queues
    /// (taken, sent or refused).
    fn fill<N: Network<P::Memory>, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told>(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
        taken: &mut u8,
    ) -> u8;

    /// Put the next arena on the air: the plan that went.
    fn publish<N, R: Rates<'id, P::Caps>, E, C: Told>(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Option<PpduPlan>;

    /// The queue the next MSDU would be taken from now.
    fn next_queue(&self, _seal: Seal) -> Option<QueueId>;

    /// Read the end of the form's air and settle it.
    fn ended<N: Network<P::Memory>, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told>(
        &mut self,
        _seal: Seal,
        facts: TxFacts,
        tbtt: Signal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    );

    /// End the link of `peer`: what its entry and the form hold of it goes
    /// home.
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

    /// The form's part of the data's rights comes: the arenas.
    fn give_arenas(&mut self, _seal: Seal, arenas: [IdleArena<'id>; 2]);

    /// The arenas back, when the form holds them all idle.
    fn take_arenas(&mut self, _seal: Seal) -> Option<[IdleArena<'id>; 2]>;

    /// The form's TID agreements.
    fn agreement(
        &mut self,
        _seal: Seal,
        tid: WmmUserPriority,
        size: Option<u16>,
    ) -> Result<(), Refused>;
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
> Engine<'id, P, F, T, PEERS, POOL, SINGLES, GROUPS, AGREEMENTS>
where
    F::Sender<'id, P::Caps>: Stopped,
{
    /// The engine of stopped data, taking at most `budget` MSDUs a section
    /// (at least one).
    pub fn new(budget: u8) -> Self {
        Self {
            flow: Flow::Stopped,
            budget: budget.max(1),
            peers: core::array::from_fn(|_| Entry::Free),
            owed: [Owed::default(); PEERS],
            queues: TxQueues::new(),
            sender: <F::Sender<'id, P::Caps> as Stopped>::STOPPED,
            lanes: [const { Lane::Away }; 4],
            singles: [const { Slot::Away }; SINGLES],
            command: Commanding::Away,
            pairwise: Slot::Away,
            groups: [const { Slot::Away }; GROUPS],
            agreements: [const { Slot::Away }; AGREEMENTS],
            gate: Gate::Away,
            brand: PhantomData,
        }
    }
}

/// What a search reads of a form's state and of an entry.
#[cfg(test)]
pub trait Keyed {
    fn key(&self) -> std::string::String;
}

#[cfg(test)]
impl<K: oer_protocols_ieee80211_contracts_port::Ieee80211Capabilities> Keyed for DataPlane<'_, K> {
    fn key(&self) -> std::string::String {
        DataPlane::key(self)
    }
}

#[cfg(test)]
impl Keyed for Aside<'_> {
    fn key(&self) -> std::string::String {
        std::string::String::from(match self {
            Self::Away => "away",
            Self::Here(_) => "here",
        })
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
> Engine<'id, P, F, T, PEERS, POOL, SINGLES, GROUPS, AGREEMENTS>
where
    F::Sender<'id, P::Caps>: Keyed,
    F::Entry<P::Link, T>: Keyed,
    P::Memory: core::fmt::Debug,
{
    /// The engine's whole state, read from its private fields, for the
    /// search that merges equal states; the TBTT it keeps for the
    /// controller is left out.
    pub(crate) fn key(&self) -> std::string::String {
        use core::fmt::Write;
        let mut key = std::format!("{:?}|{}|{:?}|", self.flow, self.budget, self.owed);
        for entry in &self.peers {
            let _ = match entry {
                Entry::Free => write!(key, "free;"),
                Entry::Linked {
                    link,
                    carrying,
                    hold,
                } => write!(key, "{}|{carrying:?}|{hold:?};", link.key()),
            };
        }
        let _ = write!(key, "{}|{}|", self.queues.key(), self.sender.key());
        for lane in &self.lanes {
            let _ = match lane {
                Lane::Away => write!(key, "away;"),
                Lane::Free(_) => write!(key, "free;"),
                Lane::Air(flight) => write!(
                    key,
                    "air{}|{:?}|{};",
                    flight.peer, flight.queue, flight.psdu
                ),
            };
        }
        key.extend(self.singles.iter().map(Slot::mark));
        key.push(self.pairwise.mark());
        key.extend(self.groups.iter().map(Slot::mark));
        key.extend(self.agreements.iter().map(Slot::mark));
        key.push(match self.command {
            Commanding::Away => 'A',
            Commanding::Here(_) => 'H',
            Commanding::CloseGate(_) => 'C',
        });
        key.push(match self.gate {
            Gate::Away => 'w',
            Gate::Open(_) => 'o',
            Gate::Closing => 'g',
            Gate::Closed(_) => 'c',
        });
        key
    }
}

/// A form's state of stopped data.
pub trait Stopped {
    const STOPPED: Self;
}

impl<K: oer_protocols_ieee80211_contracts_port::Ieee80211Capabilities> Stopped
    for DataPlane<'_, K>
{
    const STOPPED: Self = DataPlane::new();
}

impl Stopped for Aside<'_> {
    const STOPPED: Self = Self::Away;
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
> Engine<'id, P, F, T, PEERS, POOL, SINGLES, GROUPS, AGREEMENTS>
where
    Self: Sections<'id, P, T, PEERS>,
    P::Ack: Copy,
    P::Denial: Copy,
{
    /// The data flows, with its rights. Nothing waits for it: a stop ended
    /// every link, and what the network offered without one was refused.
    pub fn flows(&mut self, rights: Rights<'id, SINGLES, GROUPS, AGREEMENTS>) -> Rest {
        let DataRights {
            arenas,
            singles,
            queues,
            command,
            pairwise,
            groups,
            agreements,
        } = rights;
        self.give_arenas(Seal(()), arenas);
        self.singles = singles.map(Slot::Here);
        // QUEUES' order is voice, video, best effort, background; the lanes
        // are in the categories' selection order.
        let [voice, video, best_effort, background] = queues;
        self.lanes = [best_effort, background, video, voice].map(Lane::Free);
        self.command = Commanding::Here(command);
        self.pairwise = Slot::Here(pairwise);
        self.groups = groups.map(Slot::Here);
        self.agreements = agreements.map(Slot::Here);
        self.flow = Flow::Flowing;
        Rest::Wake(None)
    }

    /// Whether the data flows.
    pub fn is_flowing(&self) -> bool {
        self.flow == Flow::Flowing
    }

    /// Whether `peer` has a link.
    pub fn is_linked(&self, peer: Peer<PEERS>) -> bool {
        matches!(self.entry(peer.0), Some(Entry::Linked { .. }))
    }

    /// Whether `peer`'s link is held.
    pub fn is_held(&self, peer: Peer<PEERS>) -> bool {
        matches!(
            self.entry(peer.0),
            Some(Entry::Linked {
                carrying: Carrying::Held,
                ..
            })
        )
    }

    /// Whether the gate is open.
    pub const fn gate_open(&self) -> bool {
        matches!(self.gate, Gate::Open(_))
    }

    /// The MSDUs the queues hold.
    pub fn queued(&self) -> usize {
        self.queues.len()
    }

    /// The BlockAckReq owed to `peer` for `tid`, to its window's head.
    pub fn bar_owed(&self, peer: Peer<PEERS>, tid: WmmUserPriority) -> Option<SequenceNumber> {
        self.owed
            .get(usize::from(peer.0))
            .and_then(|owed| *tid.select(&owed.bars))
    }

    /// The BlockAckReq owed to `peer` for `tid` went: it is owed no more.
    pub fn bar_sent(&mut self, peer: Peer<PEERS>, tid: WmmUserPriority) {
        if let Some(owed) = self.owed.get_mut(usize::from(peer.0)) {
            *tid.select_mut(&mut owed.bars) = None;
        }
    }

    /// The closed gate, for the controller, once its closing ended.
    pub fn take_closed_gate(&mut self) -> Option<GateClosed<'id>> {
        match core::mem::replace(&mut self.gate, Gate::Away) {
            Gate::Closed(gate) => Some(gate),
            other => {
                self.gate = other;
                None
            }
        }
    }

    fn entry(&self, peer: u8) -> Option<&Entry<F::Entry<P::Link, T>>> {
        self.peers.get(usize::from(peer))
    }

    fn entry_mut(&mut self, peer: u8) -> Option<&mut Entry<F::Entry<P::Link, T>>> {
        self.peers.get_mut(usize::from(peer))
    }

    /// Link `peer`. Its data goes while the data flows and the gate is
    /// open.
    ///
    /// # Errors
    ///
    /// The link back, with why: the data does not flow, or the peer has a
    /// link.
    pub fn link<N: Network<P::Memory>, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told>(
        &mut self,
        peer: Peer<PEERS>,
        link: F::Link<P::Link, T>,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Result<Rest, Refusal<F::Link<P::Link, T>>>
    where
        Self: Linking<'id, P, F, T, PEERS>,
    {
        if self.flow != Flow::Flowing {
            return Err((link, Refused::NotFlowing));
        }
        if self.is_linked(peer) {
            return Err((link, Refused::Linked));
        }
        let entry = self.take_link(Seal(()), link)?;
        if let Some(slot) = self.entry_mut(peer.0) {
            *slot = Entry::Linked {
                link: entry,
                carrying: Carrying::Carrying,
                hold: None,
            };
        }
        self.queues.reset_peer(peer.0);
        if let Some(owed) = self.owed.get_mut(usize::from(peer.0)) {
            *owed = Owed::default();
        }
        Ok(self.intake(turn))
    }

    /// The next number of `peer`'s `space`, for a frame of the controller:
    /// the TID's data waits until the controller says the frame that names
    /// it went ([`Request::Named`]).
    ///
    /// # Errors
    ///
    /// The peer has no link, or the link's numbers have no such space.
    pub fn name_next(
        &mut self,
        peer: Peer<PEERS>,
        space: LinkSpace,
    ) -> Result<SequenceNumber, Refused>
    where
        Self: Linking<'id, P, F, T, PEERS>,
    {
        let Some(Entry::Linked { link, hold, .. }) = self.peers.get_mut(usize::from(peer.0)) else {
            return Err(Refused::NotLinked);
        };
        let number = Self::numbers(Seal(()), link)
            .take(space)
            .ok_or(Refused::NotLinked)?;
        if let LinkSpace::Tid(tid) = space {
            *hold = Some(tid);
        }
        Ok(number)
    }

    /// A request of the controller.
    ///
    /// # Errors
    ///
    /// The request does not fit the engine's state; nothing changed.
    pub fn asked<
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told,
    >(
        &mut self,
        request: Request<PEERS>,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Result<Rest, Refused> {
        match request {
            Request::Unlink(peer) => {
                if !self.is_linked(peer) {
                    return Err(Refused::NotLinked);
                }
                self.unlink(peer.0, turn.port, turn.network, turn.events);
            }
            Request::Hold(peer) | Request::Release(peer) | Request::Named(peer) => {
                let Some(Entry::Linked { carrying, hold, .. }) =
                    self.peers.get_mut(usize::from(peer.0))
                else {
                    return Err(Refused::NotLinked);
                };
                match request {
                    Request::Hold(_) => *carrying = Carrying::Held,
                    Request::Release(_) => *carrying = Carrying::Carrying,
                    _ => *hold = None,
                }
                self.queues.unblock();
            }
            Request::TxAgreement(peer, tid, size) => {
                if !self.is_linked(peer) {
                    return Err(Refused::NotLinked);
                }
                self.agreement(Seal(()), tid, Some(size))?;
                self.queues.unblock();
            }
            Request::TxAgreementEnded(peer, tid) => {
                if !self.is_linked(peer) {
                    return Err(Refused::NotLinked);
                }
                self.agreement(Seal(()), tid, None)?;
                self.queues.unblock();
            }
            Request::CloseGate => self.close_gate(turn),
        }
        Ok(self.intake(turn))
    }

    /// Open the closed gate the controller lends: arenas and singles go.
    ///
    /// # Errors
    ///
    /// The gate back: the data does not flow, or the engine holds a gate
    /// (open, closing, or closed and not yet taken).
    pub fn open_gate<
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told,
    >(
        &mut self,
        gate: GateClosed<'id>,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Result<Rest, (GateClosed<'id>, Refused)> {
        if self.flow != Flow::Flowing {
            return Err((gate, Refused::NotFlowing));
        }
        if !matches!(self.gate, Gate::Away) {
            return Err((gate, Refused::GateHeld));
        }
        self.gate = Gate::Open(turn.port.open_gate(gate));
        Ok(self.intake(turn))
    }

    /// The network has MSDUs.
    pub fn offered<
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told,
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
        C: Told,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        self.poll_command(turn);
        self.intake(turn)
    }

    /// The port's transmit facts: the arena's end and the singles', then
    /// the command in progress, then the intake.
    pub fn transmitted<
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told,
    >(
        &mut self,
        facts: TxFacts,
        tbtt: Signal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        self.ended(Seal(()), facts, tbtt, turn);
        self.poll_command(turn);
        self.intake(turn)
    }

    /// Stop the data: every link ends, its queued and unpublished data goes
    /// home, an open gate closes. The rights come back once nothing is on
    /// the air and the gate's closing ended; until then the answer is
    /// [`DataStop::Going`] and the transmit facts wake.
    ///
    /// # Errors
    ///
    /// The data was not flowing: the installation holds its rights.
    pub fn stop_data<
        N: Network<P::Memory>,
        R: Rates<'id, P::Caps>,
        E: EventSink<DataEvent>,
        C: Told,
    >(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Result<DataStop<Rights<'id, SINGLES, GROUPS, AGREEMENTS>>, NotFlowing> {
        match self.flow {
            Flow::Stopped => return Err(NotFlowing),
            Flow::Flowing => {
                for peer in 0..PEERS {
                    if let Ok(peer) = u8::try_from(peer)
                        && matches!(self.entry(peer), Some(Entry::Linked { .. }))
                    {
                        self.unlink(peer, turn.port, turn.network, turn.events);
                    }
                }
                self.flow = Flow::Stopping;
                self.close_gate(turn);
                // With no link, what the network still offers goes back.
                self.drain(turn);
            }
            Flow::Stopping => self.poll_command(turn),
        }
        Ok(self.finish())
    }

    /// The rights, when nothing is on the air and no command runs.
    fn finish(&mut self) -> DataStop<Rights<'id, SINGLES, GROUPS, AGREEMENTS>> {
        let quiet = self.in_flight(Seal(())) == 0
            && matches!(self.command, Commanding::Here(_))
            && !matches!(self.gate, Gate::Open(_) | Gate::Closing)
            && self.lanes.iter().all(|lane| matches!(lane, Lane::Free(_)))
            && self.singles.iter().all(Slot::is_here)
            && self.pairwise.is_here()
            && self.groups.iter().all(Slot::is_here)
            && self.agreements.iter().all(Slot::is_here);
        if !quiet {
            return DataStop::Going;
        }
        let Some(arenas) = self.take_arenas(Seal(())) else {
            return DataStop::Going;
        };
        // Each part is here, as checked above; a part that is not puts every
        // part back as it was.
        let parts = (
            gather(&mut self.singles),
            gather(&mut self.groups),
            gather(&mut self.agreements),
            core::mem::replace(&mut self.command, Commanding::Away),
            core::mem::replace(&mut self.pairwise, Slot::Away),
            core::mem::replace(&mut self.lanes, [const { Lane::Away }; 4]),
        );
        let (
            Some(singles),
            Some(groups),
            Some(agreements),
            Commanding::Here(command),
            Slot::Here(pairwise),
            [
                Lane::Free(best_effort),
                Lane::Free(background),
                Lane::Free(video),
                Lane::Free(voice),
            ],
        ) = parts
        else {
            let (singles, groups, agreements, command, pairwise, lanes) = parts;
            if let Some(singles) = singles {
                self.singles = singles.map(Slot::Here);
            }
            if let Some(groups) = groups {
                self.groups = groups.map(Slot::Here);
            }
            if let Some(agreements) = agreements {
                self.agreements = agreements.map(Slot::Here);
            }
            self.command = command;
            self.pairwise = pairwise;
            self.lanes = lanes;
            self.give_arenas(Seal(()), arenas);
            return DataStop::Going;
        };
        self.flow = Flow::Stopped;
        DataStop::Stopped(DataRights {
            arenas,
            singles,
            queues: [voice, video, best_effort, background],
            command,
            pairwise,
            groups,
            agreements,
        })
    }

    /// End `peer`'s link.
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
        if let Some(entry) = self.entry_mut(peer) {
            *entry = Entry::Free;
        }
        if let Some(owed) = self.owed.get_mut(usize::from(peer)) {
            *owed = Owed::default();
        }
    }

    /// Ask the gate's closing: nothing new goes from now.
    fn close_gate<N, R, E, C: Told>(&mut self, turn: &mut Turn<'_, P, N, R, E, C>) {
        // The command's place is free while no command runs: the gate's
        // closing is the one command of this half.
        if !matches!(
            (&self.gate, &self.command),
            (Gate::Open(_), Commanding::Here(_))
        ) {
            return;
        }
        match (
            core::mem::replace(&mut self.gate, Gate::Closing),
            core::mem::replace(&mut self.command, Commanding::Away),
        ) {
            (Gate::Open(open), Commanding::Here(place)) => {
                self.command = Commanding::CloseGate(turn.port.begin(
                    place,
                    Command::CloseGate(open),
                    turn.now,
                ));
            }
            (gate, command) => {
                self.gate = gate;
                self.command = command;
            }
        }
    }

    /// Poll the command in progress.
    fn poll_command<N, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told>(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) {
        let command = match core::mem::replace(&mut self.command, Commanding::Away) {
            Commanding::CloseGate(command) => command,
            other => {
                self.command = other;
                return;
            }
        };
        match turn
            .port
            .poll_command(command, turn.now, &mut ends!(self, turn))
        {
            CommandPoll::Pending(command) => self.command = Commanding::CloseGate(command),
            CommandPoll::Ended(place, end) => {
                self.command = Commanding::Here(place);
                if let CommandEnd::GateClosed(gate) = end {
                    self.gate = Gate::Closed(gate);
                }
            }
        }
    }

    /// What the network offers, into the queues while the pool has room;
    /// what has no link goes back at once. Whether it moved anything.
    fn drain<N: Network<P::Memory>, R, E: EventSink<DataEvent>, C: Told>(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> bool {
        let mut moved = false;
        while self.queues.has_room() {
            let Some(msdu) = turn.network.take() else {
                break;
            };
            moved = true;
            match self.route(Seal(()), turn.network, &msdu) {
                Ok(queue) => {
                    if let Err(msdu) = self.queues.push(queue, msdu) {
                        // The pool had room: this does not happen.
                        report(turn.events, || DataEvent::DataLost(DataLoss::Overflow));
                        turn.network.home(msdu.memory);
                    }
                }
                Err(reason) => {
                    report(turn.events, || DataEvent::MsduRefused(reason));
                    turn.network.home(msdu.memory);
                }
            }
        }
        moved
    }

    /// The intake: drain, fill, publish, again while it changed something,
    /// within one budget. A round is owed when the budget ran out with
    /// more to take.
    fn intake<N: Network<P::Memory>, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told>(
        &mut self,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Rest {
        let mut taken = 0;
        loop {
            let drained = self.drain(turn);
            if self.flow != Flow::Flowing {
                break;
            }
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

/// The seal of the forms' halves: only this crate makes one, so only the
/// engine's sections call them.
#[derive(Debug)]
pub struct Seal(());

/// The form's part of a link request.
pub trait Linking<'id, P: DataPort<'id>, F: Form, T, const PEERS: usize> {
    /// Take the link into the form; its entry.
    ///
    /// # Errors
    ///
    /// The link back: the form cannot take it.
    fn take_link(&mut self, _seal: Seal, link: F::Link<P::Link, T>) -> Linked<F, P::Link, T>;

    /// The entry's numbers.
    fn numbers(_seal: Seal, entry: &mut F::Entry<P::Link, T>) -> &mut TxNumbers<T>;
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
> Linking<'id, P, Arenas, T, 1>
    for Engine<'id, P, Arenas, T, 1, POOL, SINGLES, GROUPS, AGREEMENTS>
{
    fn take_link(
        &mut self,
        _seal: Seal,
        link: StationLink<P::Link, T>,
    ) -> Result<StationEntry<P::Link, T>, (StationLink<P::Link, T>, Refused)> {
        let StationLink {
            data,
            port,
            numbers,
        } = link;
        match self.sender.handover(data) {
            Ok(()) => Ok(StationEntry { port, numbers }),
            Err((data, _)) => Err((
                StationLink {
                    data,
                    port,
                    numbers,
                },
                Refused::PlaneLinked,
            )),
        }
    }

    fn numbers(_seal: Seal, entry: &mut StationEntry<P::Link, T>) -> &mut TxNumbers<T> {
        &mut entry.numbers
    }
}

impl<
    'id,
    P: DataPort<'id>,
    T: TxKey,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> Sections<'id, P, T, 1> for Engine<'id, P, Arenas, T, 1, POOL, SINGLES, GROUPS, AGREEMENTS>
where
    P::Ack: Copy,
    P::BlockAck: Copy,
    P::Denial: Copy,
{
    fn route<N: Network<P::Memory>>(
        &self,
        _seal: Seal,
        _network: &N,
        msdu: &OutboundMsdu<P::Memory>,
    ) -> Result<QueueId, IntakeRefusal> {
        match self.peers.first() {
            Some(Entry::Linked { .. }) => {}
            _ => return Err(IntakeRefusal::NoLink),
        }
        self.sender
            .tid(msdu.priority)
            .map(QueueId::of_tid)
            .ok_or(IntakeRefusal::NoAdmittedAccessCategory)
    }

    fn fill<N: Network<P::Memory>, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told>(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
        taken: &mut u8,
    ) -> u8 {
        let Self {
            peers,
            queues,
            sender,
            budget,
            ..
        } = self;
        let Some(Entry::Linked {
            link,
            carrying: Carrying::Carrying,
            hold,
        }) = peers.first_mut()
        else {
            return 0;
        };
        let hold = hold.map(QueueId::of_tid);
        let mut moved: u8 = 0;
        while *taken < *budget {
            let queue = match sender.standby_tid() {
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
            match sender.intake(turn.port, &link.port, msdu, &mut link.numbers, turn.now) {
                Intake::Taken(_) => moved = moved.saturating_add(1),
                Intake::Wait(msdu, why) => {
                    let back = queues.push_front(queue, msdu);
                    if let Err(msdu) = back {
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

    fn publish<N, R: Rates<'id, P::Caps>, E, C: Told>(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Option<PpduPlan> {
        let Gate::Open(gate) = &self.gate else {
            return None;
        };
        let Some(Entry::Linked {
            carrying: Carrying::Carrying,
            ..
        }) = self.peers.first()
        else {
            return None;
        };
        let rates = &mut *turn.rates;
        self.sender
            .publication(turn.port, gate, turn.now, |plan| rates.arena(0, plan))
    }

    fn next_queue(&self, _seal: Seal) -> Option<QueueId> {
        if self.sender.takes() {
            self.queues.select(None)
        } else {
            None
        }
    }

    fn ended<N: Network<P::Memory>, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told>(
        &mut self,
        _seal: Seal,
        facts: TxFacts,
        tbtt: Signal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) {
        let gate = match &self.gate {
            Gate::Open(gate) => Some(gate),
            _ => None,
        };
        let rates = &mut *turn.rates;
        let network = &mut *turn.network;
        let settled = self.sender.completed(
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
            self.queues
                .charge(QueueId::of_tid(settled.tid), settled.cost);
            if let (Some((tid, head)), Some(owed)) = (settled.bar, self.owed.first_mut()) {
                *tid.select_mut(&mut owed.bars) = Some(head);
            }
            self.queues.unblock();
        }
        turn.port
            .serve(facts, tbtt, turn.now, &mut ends!(self, turn));
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
        let _data: Option<DataLink> = self
            .sender
            .stop(port, |memory| network.home(memory), events);
    }

    fn in_flight(&self, _seal: Seal) -> usize {
        usize::from(self.sender.on_air())
    }

    fn give_arenas(&mut self, _seal: Seal, arenas: [IdleArena<'id>; 2]) {
        self.sender.give(arenas);
    }

    fn take_arenas(&mut self, _seal: Seal) -> Option<[IdleArena<'id>; 2]> {
        self.sender.take()
    }

    fn agreement(
        &mut self,
        _seal: Seal,
        tid: WmmUserPriority,
        size: Option<u16>,
    ) -> Result<(), Refused> {
        let Some(Entry::Linked { link, .. }) = self.peers.first() else {
            return Err(Refused::NotLinked);
        };
        let window = size.map(|size| TxWindow {
            head: match self.sender.oldest_held(tid) {
                Some(oldest) => oldest,
                None => link.numbers.next(tid),
            },
            size,
        });
        self.sender.agreement(tid, window);
        Ok(())
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
> Engine<'id, P, Arenas, T, 1, POOL, SINGLES, GROUPS, AGREEMENTS>
{
    /// The port's A-MPDU limits at the rate the aggregate rate selection
    /// gives now: the arenas' fill follows them.
    pub fn aggregate_rate(&mut self, rate: Option<crate::plane::AggregateRate>) {
        self.sender.aggregate_rate(rate);
    }
}

/// An access point: its clients' data as singles.
impl<
    'id,
    P: DataPort<'id>,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> Linking<'id, P, Singles, T, PEERS>
    for Engine<'id, P, Singles, T, PEERS, POOL, SINGLES, GROUPS, AGREEMENTS>
{
    fn take_link(
        &mut self,
        _seal: Seal,
        link: ClientLink<T>,
    ) -> Result<ClientLink<T>, (ClientLink<T>, Refused)> {
        Ok(link)
    }

    fn numbers(_seal: Seal, entry: &mut ClientLink<T>) -> &mut TxNumbers<T> {
        &mut entry.numbers
    }
}

impl<
    'id,
    P: DataPort<'id>,
    T,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> Engine<'id, P, Singles, T, PEERS, POOL, SINGLES, GROUPS, AGREEMENTS>
{
    /// The single's place of the access point's data on the queue of
    /// `category`: the last [`DATA_SINGLES`] places, in `QUEUES`' order.
    const fn data_place(category: WmmAccessCategory) -> u8 {
        const {
            if SINGLES < DATA_SINGLES || SINGLES > u8::MAX as usize {
                panic!("an access point's layout holds a single's place a queue");
            }
        }
        // The check above bounds both.
        let first = SINGLES.saturating_sub(DATA_SINGLES) as u8;
        let offset = match category {
            WmmAccessCategory::Voice => 0,
            WmmAccessCategory::Video => 1,
            WmmAccessCategory::BestEffort => 2,
            WmmAccessCategory::Background => 3,
        };
        first.saturating_add(offset)
    }
}

/// The sink of an engine's singles' ends, over the engine's fields and
/// the turn's.
macro_rules! ends {
    ($engine:expr, $turn:expr) => {
        SingleEnds {
            lanes: &mut $engine.lanes,
            singles: &mut $engine.singles,
            queues: &mut $engine.queues,
            rates: &mut *$turn.rates,
            events: &mut *$turn.events,
            told: &mut *$turn.told,
            now: $turn.now,
        }
    };
}
use ends;

/// The sink of the singles' ends and the TBTT: an access point's data's
/// singles settle; a single of the controller gives its place and its
/// queue's right back. The engine holds every queue's right while the data
/// flows, so the port gives back only what the engine lent it.
struct SingleEnds<
    'a,
    'id,
    K,
    R,
    E,
    C,
    M,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
> {
    lanes: &'a mut [Lane<'id, K>; 4],
    singles: &'a mut [Slot<SinglePlace<'id>>; SINGLES],
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
    C: Told,
    M,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
> PortSink<'id, A, D> for SingleEnds<'_, 'id, K, R, E, C, M, PEERS, POOL, SINGLES>
{
    fn single_ended(
        &mut self,
        outcome: SingleOutcome<A, D>,
        place: SinglePlace<'id>,
        queue: QueueRight<'id>,
    ) {
        let lane = queue.category().select_mut(self.lanes);
        let Lane::Air(flight) = core::mem::replace(lane, Lane::Free(queue)) else {
            // Not the data's single: its place goes back where it was.
            if let Some(slot) = self.singles.get_mut(usize::from(place.index())) {
                *slot = Slot::Here(place);
            }
            return;
        };
        if let Some(slot) = self.singles.get_mut(usize::from(place.index())) {
            *slot = Slot::Here(place);
        }
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

impl<
    'id,
    P: DataPort<'id>,
    T: TxKey,
    const PEERS: usize,
    const POOL: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> Sections<'id, P, T, PEERS>
    for Engine<'id, P, Singles, T, PEERS, POOL, SINGLES, GROUPS, AGREEMENTS>
where
    P::Ack: Copy,
    P::Denial: Copy,
{
    fn route<N: Network<P::Memory>>(
        &self,
        _seal: Seal,
        network: &N,
        msdu: &OutboundMsdu<P::Memory>,
    ) -> Result<QueueId, IntakeRefusal> {
        let frame = network.frame(&msdu.memory, msdu.length);
        let Some(&destination) = frame.first_chunk::<6>() else {
            return Err(IntakeRefusal::ShortFrame);
        };
        let peer = self
            .peers
            .iter()
            .zip(0_u8..)
            .find_map(|(entry, peer)| match entry {
                Entry::Linked { link, .. } if link.client.get() == destination => {
                    Some((peer, link))
                }
                _ => None,
            });
        let Some((peer, link)) = peer else {
            return Err(IntakeRefusal::NoLink);
        };
        let tid = match link.qos {
            PeerQos::Qos => msdu.priority,
            PeerQos::NonQos => WmmUserPriority::UP0,
        };
        QueueId::new(peer, tid).ok_or(IntakeRefusal::NoLink)
    }

    fn fill<N: Network<P::Memory>, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told>(
        &mut self,
        _seal: Seal,
        turn: &mut Turn<'_, P, N, R, E, C>,
        taken: &mut u8,
    ) -> u8 {
        let mut moved: u8 = 0;
        while *taken < self.budget {
            let Gate::Open(_) = &self.gate else {
                break;
            };
            let Some(queue) = self.queues.select(None) else {
                break;
            };
            let tid = queue.tid();
            let category = tid.access_category();
            let free = matches!(category.select(&self.lanes), Lane::Free(_));
            let carries = matches!(
                self.entry(queue.peer()),
                Some(Entry::Linked { carrying: Carrying::Carrying, hold, .. }) if *hold != Some(tid)
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

    fn publish<N, R: Rates<'id, P::Caps>, E, C: Told>(
        &mut self,
        _seal: Seal,
        _turn: &mut Turn<'_, P, N, R, E, C>,
    ) -> Option<PpduPlan> {
        // A single goes as it is written: no arena is published.
        None
    }

    fn next_queue(&self, _seal: Seal) -> Option<QueueId> {
        self.queues.select(None).filter(|queue| {
            matches!(
                queue.tid().access_category().select(&self.lanes),
                Lane::Free(_)
            )
        })
    }

    fn ended<N: Network<P::Memory>, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told>(
        &mut self,
        _seal: Seal,
        facts: TxFacts,
        tbtt: Signal,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) {
        turn.port
            .serve(facts, tbtt, turn.now, &mut ends!(self, turn));
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
        self.lanes
            .iter()
            .filter(|lane| matches!(lane, Lane::Air(_)))
            .count()
    }

    fn give_arenas(&mut self, _seal: Seal, arenas: [IdleArena<'id>; 2]) {
        self.sender = Aside::Here(arenas);
    }

    fn take_arenas(&mut self, _seal: Seal) -> Option<[IdleArena<'id>; 2]> {
        match core::mem::replace(&mut self.sender, Aside::Away) {
            Aside::Here(arenas) => Some(arenas),
            Aside::Away => None,
        }
    }

    fn agreement(
        &mut self,
        _seal: Seal,
        _tid: WmmUserPriority,
        _size: Option<u16>,
    ) -> Result<(), Refused> {
        Err(Refused::NoAgreements)
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
> Engine<'id, P, Singles, T, PEERS, POOL, SINGLES, GROUPS, AGREEMENTS>
{
    /// Write `msdu` of `queue` by copy into its queue's data place and
    /// publish it; its memory goes home at the write. An MSDU that cannot
    /// go now (its queue's lane busy, the gate not open, no rate) waits at
    /// its queue's head, the queue blocked until an end; one that can never
    /// go is refused. No number is drawn for an MSDU that does not go.
    fn send<N: Network<P::Memory>, R: Rates<'id, P::Caps>, E: EventSink<DataEvent>, C: Told>(
        &mut self,
        queue: QueueId,
        msdu: OutboundMsdu<P::Memory>,
        turn: &mut Turn<'_, P, N, R, E, C>,
    ) {
        let tid = queue.tid();
        let category = tid.access_category();
        let place = Self::data_place(category);
        let Self {
            peers,
            queues,
            lanes,
            singles,
            gate,
            ..
        } = self;
        let refuse = |turn: &mut Turn<'_, P, N, R, E, C>, memory, reason| {
            report(turn.events, || DataEvent::MsduRefused(reason));
            turn.network.home(memory);
        };
        let Some(Entry::Linked { link, .. }) = peers.get_mut(usize::from(queue.peer())) else {
            return refuse(turn, msdu.memory, IntakeRefusal::NoLink);
        };
        let ready = matches!(category.select(lanes), Lane::Free(_))
            && matches!(gate, Gate::Open(_))
            && singles
                .iter()
                .any(|slot| matches!(slot, Slot::Here(held) if held.index() == place));
        let plan = if ready {
            turn.rates.single(queue.peer(), tid)
        } else {
            None
        };
        let Some(plan) = plan else {
            // It waits at its queue's head until an end unblocks it.
            if let Err(msdu) = queues.push_front(queue, msdu) {
                report(turn.events, || DataEvent::DataLost(DataLoss::Overflow));
                turn.network.home(msdu.memory);
            }
            queues.block(queue);
            return;
        };
        let frame = turn.network.frame(&msdu.memory, msdu.length);
        let Some((msdu_header, payload)) = MsduHeader::parse(frame) else {
            return refuse(turn, msdu.memory, IntakeRefusal::ShortFrame);
        };
        let encapsulation = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            link.access_point.get(),
            link.access_point.get(),
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
            // The header the encapsulation wrote names an individual
            // client and holds in the MPDU: this does not happen.
            return refuse(turn, msdu.memory, IntakeRefusal::TooLong);
        };
        // Found ready above: the gate is open, the lane free, the place
        // here; a part that is not leaves every part as it was.
        let Gate::Open(gate) = &*gate else {
            return refuse(turn, msdu.memory, IntakeRefusal::NoLink);
        };
        let lane = category.select_mut(lanes);
        let right = match core::mem::replace(lane, Lane::Away) {
            Lane::Free(right) => right,
            other => {
                *lane = other;
                return refuse(turn, msdu.memory, IntakeRefusal::NoLink);
            }
        };
        let Some(mut place) = singles.iter_mut().find_map(|slot| match slot {
            Slot::Here(held) if held.index() == place => slot.lend(),
            _ => None,
        }) else {
            *lane = Lane::Free(right);
            return refuse(turn, msdu.memory, IntakeRefusal::NoLink);
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
        // An MPDU's length is far below `u32::MAX`.
        let psdu =
            u32::try_from(length.get()).map_or(u32::MAX, |mpdu| mpdu.saturating_add(FCS_LEN));
        *lane = Lane::Air(InFlight {
            peer: queue.peer(),
            queue,
            plan,
            psdu,
        });
    }
}

#[cfg(test)]
mod tests;
