//! The data path (`design/port-contract-2026-11-16.md`, section 4):
//! chains of frames filled and published once on a queue, commands, and
//! the facts that wake their caller.
//!
//! No call returns an error. A call that needs a state takes its right,
//! each a value of at most two words branded with the installation, built
//! by this crate alone: the port makes the next right with its
//! [`PortKey`](crate::life::PortKey).

use core::marker::PhantomData;
use core::task::Context;
use oer_protocols_ieee802_wire::IndividualAddress;

use oer_portable_bounded::Index;
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_wire::ccmp::GroupKeyId;
use oer_protocols_ieee80211_wire::qos::{Tid, WmmAccessCategory};
use oer_protocols_ieee80211_wire::security::Spp;
use oer_protocols_radio_port::ClockError;

use crate::life::{PortKey, RadioPort};
use crate::{
    AggregateOutcome, AmpduRate, GateClosed, GateOpen, Ieee80211Instant, InterfaceRole, MpduLength,
    PortRate, Protection, RetryPlan, SingleOutcome, StationHe, SupportedBlockAck, SupportedEdca,
    TbttEvent,
};

/// The brand of one installation, invariant in `'id`.
type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

/// The transmit events a port sampled: its interrupt word, or none for a
/// service that a deadline asks for.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TxFacts {
    events: u32,
}

impl TxFacts {
    /// No event: the service a due deadline asks for.
    pub const DUE: Self = Self { events: 0 };

    /// The backend's sampled event word `bits`.
    pub const fn events(bits: u32) -> Self {
        Self { events: bits }
    }

    /// The event word.
    pub const fn bits(self) -> u32 {
        self.events
    }

    /// The events of both samples.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            events: self.events | other.events,
        }
    }
}

/// Whether one signal of the port came in a poll: one fact of the poll.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Signal {
    /// It did not.
    #[default]
    Quiet,
    /// It did.
    Raised,
}

impl Signal {
    /// Whether it did.
    pub const fn is_raised(self) -> bool {
        matches!(self, Self::Raised)
    }
}

/// What the port's signals held when its caller polled them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Facts {
    /// Transmit events to serve.
    pub tx: Option<TxFacts>,
    /// A TBTT of the station's schedule, served with the transmit events.
    pub tbtt: Signal,
    /// The receive ring has work for a round.
    pub rx: Signal,
    /// The port needs a stop: one indication with no payload, latched
    /// until the stop that follows (rule 6 of section 1).
    pub needs_stop: Signal,
}

impl Facts {
    /// Whether the signals held nothing.
    pub const fn is_empty(&self) -> bool {
        self.tx.is_none()
            && !self.tbtt.is_raised()
            && !self.rx.is_raised()
            && !self.needs_stop.is_raised()
    }
}

/// The frames of an arena by their positions, the first frame at bit 0.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct FrameSet(u64);

impl FrameSet {
    /// No frame.
    pub const EMPTY: Self = Self(0);

    /// The first `count` frames.
    pub const fn prefix(count: u8) -> Self {
        if count >= 64 {
            Self(u64::MAX)
        } else {
            Self((1_u64 << count).wrapping_sub(1))
        }
    }

    /// The frames of the mask `bits`.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// The mask.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Whether the frame at `position` is in the set.
    pub const fn contains(self, position: u8) -> bool {
        position < 64 && self.0 & (1 << position) != 0
    }

    /// The frames of the set.
    pub const fn len(self) -> u32 {
        self.0.count_ones()
    }

    /// The number of frames, which a byte holds: a set names at most 64.
    pub const fn count(self) -> u8 {
        Index::<65>::ones(self.0).to_u8()
    }

    /// Whether the set holds no frame.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The frames of both sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The frames of this set that are not in `other`.
    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

macro_rules! right {
    ($(#[$doc:meta])* $name:ident { $($field:ident: $ty:ty),* }) => {
        $(#[$doc])*
        #[must_use = "a right is the only way to the call that takes it"]
        #[derive(Debug)]
        pub struct $name<'id> {
            $($field: $ty,)*
            brand: Brand<'id>,
        }
    };
}

/// The interface a right is of, as a type.
type Of<I> = PhantomData<fn() -> I>;

/// A right of one interface `I`: it reaches only that interface's calls.
macro_rules! interface_right {
    ($(#[$doc:meta])* $name:ident { $($field:ident: $ty:ty),* }) => {
        $(#[$doc])*
        #[must_use = "a right is the only way to the call that takes it"]
        pub struct $name<'id, I> {
            $($field: $ty,)*
            brand: Brand<'id>,
            interface: Of<I>,
        }

        impl<I> core::fmt::Debug for $name<'_, I> {
            fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                formatter
                    .debug_struct(stringify!($name))
                    .field("interface", &core::any::type_name::<I>())
                    $(.field(stringify!($field), &self.$field))*
                    .finish()
            }
        }
    };
}

right!(
    /// A transmit queue, free, with its chain of one descriptor, idle: the
    /// radio's, one a queue. A chain of one is opened from it and goes on
    /// it; a long chain's publication takes it whole, its chain of one with
    /// it, idle, until the settlement gives it back. Every interface
    /// publishes on the four queues.
    Queue { category: WmmAccessCategory }
);

/// A long chain of `SLOTS` descriptors that holds nothing, of the marker
/// `M` of its layout's set ([`LongChains`]): what an installation gives
/// and a settlement or a withdrawal returns. The radio's: a chain's
/// interface is its link's.
#[must_use = "a right is the only way to the call that takes it"]
pub struct LongChain<'id, M, const SLOTS: usize> {
    pub(crate) brand: Brand<'id>,
    pub(crate) marker: PhantomData<fn() -> M>,
}

impl<M, const SLOTS: usize> core::fmt::Debug for LongChain<'_, M, SLOTS> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LongChain")
            .field("marker", &core::any::type_name::<M>())
            .field("slots", &SLOTS)
            .finish()
    }
}

/// A chain of either kind: a queue's chain of one or a long chain, with how
/// it may go and what its end may be. Only this crate implements it.
pub trait ChainKind<'id>: crate::life::sealed::Sealed + Sized {
    /// How it goes: a chain of one as one MPDU of a kind; a long chain as
    /// one MPDU or an aggregate.
    type How<K>;
    /// What its end may be: a single's outcome; or a single's or an
    /// aggregate's.
    type End<A, B, D>;
    /// Its descriptors: the frames it takes.
    const DESCRIPTORS: usize;
}

/// A chain of an installation of the layout `L`: it reaches its own `T` of
/// the port's storage ([`Buffers`]) by its type, a long chain by its
/// marker and a chain of one by its queue, with no index. Only this crate
/// implements it.
pub trait ChainOf<'id, L: Layout>: ChainKind<'id> {
    /// Its `T`.
    fn buffer<'s, T>(&self, buffers: &'s mut Buffers<T, L::Longs>) -> &'s mut T;
    /// The same, to read.
    fn buffer_ref<'s, T>(&self, buffers: &'s Buffers<T, L::Longs>) -> &'s T;
}

impl crate::life::sealed::Sealed for Queue<'_> {}
impl<M, const SLOTS: usize> crate::life::sealed::Sealed for LongChain<'_, M, SLOTS> {}

impl<'id> ChainKind<'id> for Queue<'id> {
    type How<K> = OnePpdu<'id, K>;
    type End<A, B, D> = SingleOutcome<A, D>;
    const DESCRIPTORS: usize = 1;
}

impl<'id, M, const SLOTS: usize> ChainKind<'id> for LongChain<'id, M, SLOTS> {
    type How<K> = LongPpdu<'id, K>;
    type End<A, B, D> = ArenaOutcome<A, B, D>;
    const DESCRIPTORS: usize = SLOTS;
}

impl<'id, L: Layout> ChainOf<'id, L> for Queue<'id> {
    fn buffer<'s, T>(&self, buffers: &'s mut Buffers<T, L::Longs>) -> &'s mut T {
        buffers.one_mut(self.category)
    }
    fn buffer_ref<'s, T>(&self, buffers: &'s Buffers<T, L::Longs>) -> &'s T {
        buffers.one_ref(self.category)
    }
}

/// A long chain of a marker of the layout's set: the brand `'id` is one
/// installation's, whose mint gave its chains of the layout's `SLOTS`.
impl<'id, L: Layout, M: LongIn<L::Longs>, const SLOTS: usize> ChainOf<'id, L>
    for LongChain<'id, M, SLOTS>
{
    fn buffer<'s, T>(&self, buffers: &'s mut Buffers<T, L::Longs>) -> &'s mut T {
        buffers.long_mut::<M>()
    }
    fn buffer_ref<'s, T>(&self, buffers: &'s Buffers<T, L::Longs>) -> &'s T {
        buffers.long_ref::<M>()
    }
}

right!(
    /// The one place of a command: the radio's.
    CommandPlace {}
);
interface_right!(
    /// One of the interface's pairwise key places, empty.
    PairwiseSlot { index: u8 }
);
interface_right!(
    /// One of the interface's pairwise keys, installed.
    InstalledPairwise { index: u8 }
);
interface_right!(
    /// One of the interface's group key slots, empty.
    GroupSlot { index: u8 }
);
interface_right!(
    /// One of the interface's group keys, installed.
    InstalledGroup { index: u8 }
);
interface_right!(
    /// One of the interface's receive Block Ack agreements' slots, empty.
    AgreementSlot { index: u8 }
);
interface_right!(
    /// One of the interface's receive Block Ack agreements, running.
    Agreement { index: u8 }
);
interface_right!(
    /// The interface's receive rule, off: the hardware holds neither its
    /// address nor a rule of it.
    ReceiveOff {}
);
interface_right!(
    /// The interface's receive rule, set: the hardware holds its address
    /// and its rule.
    Receiving {}
);

impl Queue<'_> {
    /// The queue.
    pub const fn category(&self) -> WmmAccessCategory {
        self.category
    }
}

macro_rules! indexed {
    ($($name:ident),*) => {$(
        impl<I> $name<'_, I> {
            /// The place's index among its interface's of its kind.
            pub const fn index(&self) -> u8 {
                self.index
            }
        }
    )*};
}
indexed!(
    PairwiseSlot,
    InstalledPairwise,
    GroupSlot,
    InstalledGroup,
    AgreementSlot,
    Agreement
);

/// The access categories, a queue each, in the order of
/// [`RadioRights::queues`].
pub const QUEUES: [WmmAccessCategory; 4] = [
    WmmAccessCategory::Voice,
    WmmAccessCategory::Video,
    WmmAccessCategory::BestEffort,
    WmmAccessCategory::Background,
];

mod layout;
pub use layout::{
    Buffers, First, InterfaceLayout, InterfaceRights, Interfaces, Layout, LayoutRights, LongChains,
    LongIn, LongMarker, Member, RadioLayout, RadioRights, Second,
};
mod places;
pub use places::{
    FramePlace, FreeQueue, IdleLong, LongIs, LongNow, LongOnAir, LongPlace, LongPpdu, LongSettled,
    OneFrame, OneOnAir, OnePpdu, OpenLong, QueueIs, QueueNow, QueuePlace, QueuePlaces,
};

/// The chain takes no more frames: its descriptors are full. The one answer of
/// the path that is not an outcome; what comes next is `publish`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Complete;

/// The length of a header the engine writes into a frame's headroom.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeaderLen(u8);

/// A QoS data header of three addresses (IEEE Std 802.11-2020, 9.3.2.1:
/// Frame Control, Duration/ID, Address 1 to 3, Sequence Control, QoS
/// Control).
const QOS_DATA_HEADER: usize = 26;
/// The CCMP header (12.5.3.2).
const CCMP_HEADER: usize = 8;
/// The LLC/SNAP header of an RFC 1042 encapsulated MSDU (5.1.4).
const LLC_SNAP: usize = 8;
/// What is written in front of a body: the longest front the engine
/// writes.
const WRITTEN: usize = QOS_DATA_HEADER + CCMP_HEADER + LLC_SNAP;
/// The body's alignment in the network's memory: a word.
const BODY_ALIGNMENT: usize = 4;

/// The octets in front of a body, in the same memory: the MPDU's front
/// ([`WRITTEN`], 42) rounded up to the body's alignment (44). They hold
/// what the network wrote there (a data MSDU's Ethernet header is their
/// last 14 octets); the engine reads what it needs of that and writes the
/// MPDU's front flush against the body, and its [`HeaderLen`] says where
/// the MPDU begins. A role that writes more (an HT Control) changes it,
/// and each role's const check of its longest front says so.
pub const HEADROOM: usize = match WRITTEN % BODY_ALIGNMENT {
    0 => WRITTEN,
    rest => WRITTEN + (BODY_ALIGNMENT - rest),
};

impl HeaderLen {
    /// `len`, when it fits the headroom.
    pub const fn new(len: usize) -> Option<Self> {
        match Index::<{ HEADROOM + 1 }>::new(len) {
            Ok(octets) => Some(Self(octets.to_u8())),
            Err(_) => None,
        }
    }

    /// The length.
    pub const fn get(self) -> usize {
        self.0 as usize
    }
}

/// The room behind a body for its MIC: the longest MIC of the ciphers the
/// contract carries, CCMP's 8 octets (IEEE Std 802.11-2020, 12.5.3.2). The
/// port appends it to a protected MPDU; a cipher with a longer MIC changes
/// this line, and every slot follows.
pub const MIC_ROOM: usize = oer_protocols_ieee80211_wire::ccmp::CCMP_MIC_LEN;

/// The length of a transmit slot for a body of `body` octets: the port's
/// octets in front of the headroom ([`DataPort::TX_FRONT`]), the
/// [`HEADROOM`], the body, its MIC's room ([`MIC_ROOM`]) and the port's
/// octets behind them ([`DataPort::TX_BEHIND`]); `None` past `usize`.
pub const fn tx_slot<'id, P: DataPort<'id>>(body: usize) -> Option<usize> {
    let Some(front) = P::TX_FRONT.checked_add(HEADROOM) else {
        return None;
    };
    let Some(body) = front.checked_add(body) else {
        return None;
    };
    let Some(mic) = body.checked_add(MIC_ROOM) else {
        return None;
    };
    mic.checked_add(P::TX_BEHIND)
}

/// A transmit slot's geometry for port `P`, checked: the one value a
/// slot's owner is built from, made in a const item of the owner by
/// [`TxSlot::new`], the constructor that refuses the same numbers at run
/// time.
pub struct TxSlot<P> {
    front: usize,
    behind: usize,
    body: usize,
    len: usize,
    port: PhantomData<fn() -> P>,
}

impl<P> Clone for TxSlot<P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P> Copy for TxSlot<P> {}

/// Why a slot's geometry is refused, before any slot is built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxSlotRefused {
    /// The body would not lie on a word.
    BodyOffWord,
    /// The slot is shorter than [`tx_slot`] of its body.
    Short,
    /// The longest MPDU of the body is longer than the port transmits.
    MpduTooLong,
    /// The lengths overflow `usize`.
    Overflow,
}

impl<P> TxSlot<P> {
    /// A slot of `len` octets for bodies of at most `body` octets, when:
    ///
    /// - the body lies on a word (`BODY_ALIGNMENT`): the network writes an
    ///   MSDU's payload, an IP datagram, from there and reads its header
    ///   in words, as in its own memory;
    /// - `len` holds [`tx_slot`] of `body`: the port's octets in front,
    ///   the headroom, the body, its MIC's room and the port's octets
    ///   behind;
    /// - the longest MPDU of such a body is one `P` transmits: the longest
    ///   front the engine writes (`WRITTEN`: header, CCMP header,
    ///   LLC/SNAP) and the body, against the capability's
    ///   `max_mpdu_length`, which counts the MPDU without its MIC and FCS
    ///   (IEEE Std 802.11-2020, 9.2.3: an MPDU is its header, its body
    ///   and its FCS; 12.5.3.2: the MIC follows the data inside the body).
    ///
    /// # Errors
    ///
    /// [`TxSlotRefused`] names the relation that does not hold.
    pub const fn new<'id>(body: usize, len: usize) -> Result<Self, TxSlotRefused>
    where
        P: DataPort<'id>,
    {
        let Some(at) = P::TX_FRONT.checked_add(HEADROOM) else {
            return Err(TxSlotRefused::Overflow);
        };
        if at % BODY_ALIGNMENT != 0 {
            return Err(TxSlotRefused::BodyOffWord);
        }
        let Some(needed) = tx_slot::<P>(body) else {
            return Err(TxSlotRefused::Overflow);
        };
        if needed > len {
            return Err(TxSlotRefused::Short);
        }
        let Some(mpdu) = WRITTEN.checked_add(body) else {
            return Err(TxSlotRefused::Overflow);
        };
        if mpdu > <P::Caps as crate::Ieee80211Capabilities>::CAPS.max_mpdu_length as usize {
            return Err(TxSlotRefused::MpduTooLong);
        }
        Ok(Self {
            front: P::TX_FRONT,
            behind: P::TX_BEHIND,
            body,
            len,
            port: PhantomData,
        })
    }

    /// The slot's octets.
    pub const fn len(self) -> usize {
        self.len
    }

    /// Where the headroom begins: behind the port's octets in front.
    pub const fn headroom_at(self) -> usize {
        self.front
    }

    /// Where the body begins: behind the headroom, on a word.
    pub const fn body_at(self) -> usize {
        // Inside `len`, which `new` found in `usize`.
        self.front.saturating_add(HEADROOM)
    }

    /// The port's octets in front of the headroom.
    pub const fn front(self) -> core::ops::Range<usize> {
        0..self.front
    }

    /// The port's octets behind a body of `body` octets and its MIC's
    /// room, when the slot holds such a body.
    pub const fn behind(self, body: usize) -> Option<core::ops::Range<usize>> {
        if body > self.body {
            None
        } else {
            // Inside `len`, which `new` found in `usize`.
            let at = self.body_at().saturating_add(body).saturating_add(MIC_ROOM);
            Some(at..at.saturating_add(self.behind))
        }
    }
}

/// The slot of `LEN` octets for bodies of at most `BODY` of port `P`,
/// checked at build: what a slot's owner names in its own const item
/// ([`Self::SLOT`]); a geometry [`TxSlot::new`] refuses does not build.
pub struct TxSlotOf<'id, P, const BODY: usize, const LEN: usize> {
    port: PhantomData<(Brand<'id>, fn() -> P)>,
}

impl<'id, P: DataPort<'id>, const BODY: usize, const LEN: usize> TxSlotOf<'id, P, BODY, LEN> {
    /// The checked slot.
    pub const SLOT: TxSlot<P> = match TxSlot::new(BODY, LEN) {
        Ok(slot) => slot,
        Err(TxSlotRefused::BodyOffWord) => {
            panic!("the body lies on a word: the port's front and the headroom are whole words")
        }
        Err(TxSlotRefused::Short) => {
            panic!(
                "a slot holds the port's octets, the headroom, the body, its MIC and the port's octets behind"
            )
        }
        Err(TxSlotRefused::MpduTooLong) => {
            panic!("the longest MPDU of the body is one the port transmits")
        }
        Err(TxSlotRefused::Overflow) => panic!("a slot's lengths fit usize"),
    };
}

/// An aggregate's publication.
pub struct AggregatePpdu<'id, K> {
    /// The aggregate's rate.
    pub rate: AmpduRate<'id, K>,
    /// The recipient's Minimum MPDU Start Spacing, the peer's for the
    /// link's life.
    pub spacing: oer_protocols_ieee80211_wire::ht::MinimumMpduStartSpacing,
    /// The control exchange that protects the aggregate, as the upper MAC's
    /// policy decided it this publication.
    pub protection: Protection,
    /// The plan of a prefix of one frame.
    pub alone: RetryPlan<PortRate<'id, K>>,
}

impl<K> Clone for AggregatePpdu<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K> Copy for AggregatePpdu<'_, K> {}

/// How a long chain's publication ended: as what went, not as what was
/// asked. An aggregate ([`LongPpdu::Aggregate`]) whose prefix was one frame
/// ends `Single`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArenaOutcome<A, B, D> {
    /// One MPDU went.
    Single(SingleOutcome<A, D>),
    /// An A-MPDU went.
    Aggregate(AggregateOutcome<B, D>),
}

/// A chain's end, kept by the port and read in place through the
/// reference [`DataPort::ended`] returns: what the prefix's frames
/// did on the air.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ended<E> {
    /// What went, and how it ended: a chain of one's single outcome, a
    /// long chain's [`ArenaOutcome`].
    pub outcome: E,
    /// The frames of the prefix that went.
    pub sent: FrameSet,
    /// The frames the response acknowledged.
    pub acknowledged: FrameSet,
}

/// A command in progress: its kind and the rights its end returns, `H`
/// (the gate, a slot, an installed key or agreement), the port's until its
/// end, which is its kind's ([`CommandPoll`]). The command's values (a key,
/// a peer, parameters) are consumed when it begins: a port that needs one
/// across polls keeps it itself. Only a port's key makes one
/// (`PortKey::began`).
#[must_use = "a right is the only way to the call that takes it"]
#[derive(Debug)]
pub struct InCommand<'id, H> {
    held: H,
    brand: Brand<'id>,
}

/// A command's poll: in progress, or ended with the command's place and its
/// kind's end.
#[derive(Debug)]
pub enum CommandPoll<'id, H, E> {
    /// In progress; the port's facts wake.
    Pending(InCommand<'id, H>),
    /// It ended, with the command's place.
    Ended(CommandPlace<'id>, E),
}

/// Close the gate, once what is on the air ended. Its end is the closed
/// gate; it has no expired end.
#[derive(Debug)]
pub struct CloseGate<'id>(pub GateOpen<'id>);

/// A command on a pairwise key place of the interface `I`.
#[derive(Debug)]
pub enum PairwiseCommand<'id, I> {
    /// Install a pairwise key in the empty place.
    Install {
        /// The empty slot.
        slot: PairwiseSlot<'id, I>,
        /// The peer.
        peer: IndividualAddress,
        /// The key.
        key: Ccmp128Key,
        /// The association's SPP A-MSDU decision.
        spp: Spp,
    },
    /// Remove the pairwise key.
    Remove(InstalledPairwise<'id, I>),
}

/// What a pairwise key command in progress holds: the slot it installs
/// in, or the key it removes.
#[derive(Debug)]
pub enum PairwiseHeld<'id, I> {
    /// An installation's empty slot.
    Installing(PairwiseSlot<'id, I>),
    /// A removal's installed key.
    Removing(InstalledPairwise<'id, I>),
}

/// A pairwise key command's end: the slot always comes back.
#[derive(Debug)]
pub enum PairwiseEnd<'id, I> {
    /// The key is installed.
    Installed(InstalledPairwise<'id, I>),
    /// The slot is empty: an installation that expired (the port cleared
    /// the entry) or a removal.
    Empty(PairwiseSlot<'id, I>),
}

/// A command on a group key slot of the interface `I`.
#[derive(Debug)]
pub enum GroupCommand<'id, I> {
    /// Install a group key in an empty slot.
    Install {
        /// The empty slot.
        slot: GroupSlot<'id, I>,
        /// The key's ID.
        key_id: GroupKeyId,
        /// The key.
        key: Ccmp128Key,
    },
    /// Replace an installed group key.
    Replace {
        /// The installed key.
        installed: InstalledGroup<'id, I>,
        /// The new key's ID.
        key_id: GroupKeyId,
        /// The new key.
        key: Ccmp128Key,
    },
    /// Make an installed group key the one received under its ID.
    Select(InstalledGroup<'id, I>),
    /// Remove a group key.
    Remove(InstalledGroup<'id, I>),
}

/// What a group key command in progress holds: its kind and its slot.
#[derive(Debug)]
pub enum GroupHeld<'id, I> {
    /// An installation's empty slot.
    Installing(GroupSlot<'id, I>),
    /// A replacement's installed key.
    Replacing(InstalledGroup<'id, I>),
    /// A selection's installed key.
    Selecting(InstalledGroup<'id, I>),
    /// A removal's installed key.
    Removing(InstalledGroup<'id, I>),
}

/// A group key command's end: the slot always comes back.
#[derive(Debug)]
pub enum GroupEnd<'id, I> {
    /// A group key is installed, replaced or selected.
    Installed(InstalledGroup<'id, I>),
    /// The slot is empty: an installation, replacement or selection that
    /// expired (the port cleared the entry), or a removal.
    Empty(GroupSlot<'id, I>),
}

/// A command on a receive Block Ack agreement's slot of the interface `I`.
#[derive(Debug)]
pub enum AgreementCommand<'id, I, K> {
    /// Begin a receive Block Ack agreement.
    Add {
        /// The empty slot.
        slot: AgreementSlot<'id, I>,
        /// The agreement.
        agreement: SupportedBlockAck<K>,
    },
    /// End a receive Block Ack agreement.
    End(Agreement<'id, I>),
}

/// What an agreement command in progress holds: the slot it adds in, or
/// the agreement it ends.
#[derive(Debug)]
pub enum AgreementHeld<'id, I> {
    /// An addition's empty slot.
    Adding(AgreementSlot<'id, I>),
    /// An end's running agreement.
    Ending(Agreement<'id, I>),
}

/// An agreement command's end: the slot always comes back.
#[derive(Debug)]
pub enum AgreementEnd<'id, I> {
    /// An agreement runs.
    Added(Agreement<'id, I>),
    /// The slot is empty: an addition that expired, or an end.
    Empty(AgreementSlot<'id, I>),
}

/// A command on the interface `I`'s receive rule.
pub enum ReceiveCommand<'id, I: InterfaceRole, K> {
    /// Set the interface's own address and its rule.
    Set {
        /// The rule, off.
        off: ReceiveOff<'id, I>,
        /// The interface's own address.
        address: IndividualAddress,
        /// The rule.
        rule: I::Receive<K>,
    },
    /// Change the rule of the set interface.
    Change {
        /// The rule, set.
        receiving: Receiving<'id, I>,
        /// The new rule.
        rule: I::Receive<K>,
    },
    /// End the rule: the address and the rule leave the hardware.
    End(Receiving<'id, I>),
}

/// What a receive command in progress holds: its kind and the rule's
/// right.
#[derive(Debug)]
pub enum ReceiveHeld<'id, I> {
    /// A setting's rule, off.
    Setting(ReceiveOff<'id, I>),
    /// A change's rule, set.
    Changing(Receiving<'id, I>),
    /// An end's rule, set.
    Ending(Receiving<'id, I>),
}

/// A receive command's end: the right always comes back, set or off.
#[derive(Debug)]
pub enum ReceiveEnd<'id, I> {
    /// The rule is set: a setting or a change took effect.
    Receiving(Receiving<'id, I>),
    /// The rule is off: an end, or a setting or a change that expired (the
    /// port cleared the address and the rule, the hardware as before the
    /// setting).
    Off(ReceiveOff<'id, I>),
}

/// A command of values: it carries no right.
pub enum ValueCommand<K> {
    /// Set the station's EDCA parameters.
    SetEdca(SupportedEdca<K>),
    /// Set the BSS color.
    SetBssColor(oer_protocols_ieee80211_wire::he::BssColor),
    /// Set the station's HE state.
    SetHe(StationHe),
}

/// What a command of values in progress holds: its kind alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueKind {
    /// The station's EDCA parameters set.
    SetEdca,
    /// The BSS color set.
    SetBssColor,
    /// The station's HE state set.
    SetHe,
}

/// A command of values' end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueEnd {
    /// It took effect.
    Done,
    /// It did not take effect within its bound.
    Expired,
}

/// What receives what a port's service tells: the TBTT. Every chain's end
/// is pulled, from its place ([`OneOnAir::ended`], [`LongOnAir::ended`]).
pub trait PortSink {
    /// A TBTT of the station's schedule.
    fn tbtt(&mut self, event: TbttEvent);
}

/// The data path, as a port gives it (section 4).
pub trait DataPort<'id>: RadioPort<'id> {
    /// The link of the interface `I` a chain is opened for: it carries
    /// its key, and names the interface the chain's frames go as.
    type Link<I: InterfaceRole>;
    /// The MSDU's memory, by its handle.
    type Memory;
    /// Octets the port's transmit memory needs in front of the
    /// [`HEADROOM`], in the same memory: what the hardware reads before
    /// the MPDU. Nothing above the port writes them.
    const TX_FRONT: usize;
    /// Octets the port's transmit memory needs behind a body and its MIC's
    /// room ([`MIC_ROOM`]), whatever the cipher: what the hardware appends.
    /// Nothing above the port writes them.
    const TX_BEHIND: usize;
    /// The evidence of a single's acknowledgement.
    type Ack;
    /// The evidence of an aggregate's BlockAck.
    type BlockAck;
    /// The air's denial.
    type Denial;

    /// The port's facts; a fact not raised registers the waker.
    fn poll_facts(&mut self, cx: &mut Context<'_>) -> Facts;

    /// The earliest instant at which a call must run without a fact.
    fn deadline(&self) -> Option<Instant>;

    /// The channel the port runs on.
    fn channel(&self) -> crate::Channel;

    /// What the port aggregates.
    fn ampdu_capabilities(&self) -> crate::AmpduCapabilities;

    /// The rate, when the port's capabilities send it on its channel: a
    /// check of the caller's value. Only this mints a [`PortRate`], which
    /// holds for the installation.
    fn rate(&self, rate: crate::PhyRate) -> Option<PortRate<'id, Self::Caps>> {
        if <Self::Caps as crate::Ieee80211Capabilities>::CAPS.supports_rate(rate, self.channel()) {
            Some(PortRate::checked(rate))
        } else {
            None
        }
    }

    /// The rate, when the port aggregates at it.
    fn ampdu_rate(&self, rate: PortRate<'id, Self::Caps>) -> Option<AmpduRate<'id, Self::Caps>> {
        let capabilities = self.ampdu_capabilities();
        (capabilities.formats.contains_rate(rate.get())
            && capabilities.max_length.at(rate.get()).is_ok())
        .then(|| AmpduRate::checked(rate))
    }

    /// A reading of the port's radio clock.
    ///
    /// # Errors
    ///
    /// The clock cannot be read now.
    fn sample_clock(&mut self) -> Result<Ieee80211Instant, ClockError>;

    /// The chain primitives follow, each sealed ([`Sealed`]): a core
    /// reaches them only through the places' transitions
    /// ([`QueuePlace`], [`LongPlace`]). Open `chain` for `link` of the
    /// interface `I` and `tid`.
    fn open_chain<I: InterfaceRole, C: ChainOf<'id, Self::Layout>>(
        &mut self,
        token: Sealed,
        chain: &C,
        link: &Self::Link<I>,
        tid: Tid,
    );

    /// Whether the open long chain `chain` of `frames` has a place for a
    /// frame of `length`: [`Complete`] when its descriptors are full.
    ///
    /// # Errors
    ///
    /// The chain's descriptors are full.
    fn place_frame<C: ChainOf<'id, Self::Layout>>(
        &mut self,
        token: Sealed,
        chain: &C,
        frames: u8,
        length: MpduLength<Self::Caps>,
    ) -> Result<(), Complete>;

    /// Write a frame into its place in `chain`: the body by its handle, in
    /// place, and the header once into the headroom the port lends.
    fn write_frame<C: ChainOf<'id, Self::Layout>>(
        &mut self,
        token: Sealed,
        chain: &C,
        body: Self::Memory,
        header: impl FnOnce(&mut [u8; HEADROOM]) -> HeaderLen,
    );

    /// Publish the chain of one of `queue`, its one frame, as `how` says.
    fn publish_one(
        &mut self,
        token: Sealed,
        queue: &Queue<'id>,
        how: OnePpdu<'id, Self::Caps>,
        gate: &GateOpen<'id>,
        now: Instant,
    );

    /// Publish the long chain `chain` on `queue` as `how` says: a single
    /// its first frame; an aggregate the longest prefix of its frames the
    /// rate admits, one frame at least. How many went.
    fn publish_long<M: LongIn<<Self::Layout as Layout>::Longs>, const SLOTS: usize>(
        &mut self,
        token: Sealed,
        chain: &LongChain<'id, M, SLOTS>,
        queue: &Queue<'id>,
        how: LongPpdu<'id, Self::Caps>,
        gate: &GateOpen<'id>,
        now: Instant,
    ) -> u8;

    /// The end of the chain of one of `queue`, read in place, once it
    /// ended: a single's outcome.
    fn ended_one(
        &mut self,
        token: Sealed,
        queue: &Queue<'id>,
        facts: TxFacts,
        now: Instant,
    ) -> Option<&Ended<SingleOutcome<Self::Ack, Self::Denial>>>;

    /// The end of the long chain `chain`, whose first `sent` frames went,
    /// read in place, once it ended.
    fn ended_long<M: LongIn<<Self::Layout as Layout>::Longs>, const SLOTS: usize>(
        &mut self,
        token: Sealed,
        chain: &LongChain<'id, M, SLOTS>,
        sent: u8,
        facts: TxFacts,
        now: Instant,
    ) -> Option<&Ended<ArenaOutcome<Self::Ack, Self::BlockAck, Self::Denial>>>;

    /// Settle the ended `chain`, whose first `sent` frames went: the frames
    /// of `keep` and those that did not go stay in their places, every other
    /// body goes home. How many stay.
    fn settle_chain<C: ChainOf<'id, Self::Layout>>(
        &mut self,
        token: Sealed,
        chain: &C,
        sent: u8,
        keep: FrameSet,
        home: &mut impl FnMut(Self::Memory),
    ) -> u8;

    /// Settle the ended chain of one of `queue`: its one frame went, and
    /// its body goes home.
    fn settle_one(
        &mut self,
        token: Sealed,
        queue: &Queue<'id>,
        home: &mut impl FnMut(Self::Memory),
    );

    /// Withdraw the frames of the open `chain`: each body goes home.
    fn withdraw_chain<C: ChainOf<'id, Self::Layout>>(
        &mut self,
        token: Sealed,
        chain: &C,
        home: &mut impl FnMut(Self::Memory),
    );

    /// Serve the transmit facts: the queues' next, the TBTT.
    fn serve(&mut self, facts: TxFacts, tbtt: Signal, now: Instant, sink: &mut impl PortSink);

    /// Begin closing the gate.
    fn begin_close(
        &mut self,
        place: CommandPlace<'id>,
        command: CloseGate<'id>,
        now: Instant,
    ) -> InCommand<'id, GateOpen<'id>>;

    /// Poll the gate's closing.
    fn poll_close(
        &mut self,
        command: InCommand<'id, GateOpen<'id>>,
        now: Instant,
        sink: &mut impl PortSink,
    ) -> CommandPoll<'id, GateOpen<'id>, GateClosed<'id>>;

    /// Begin a pairwise key command.
    fn begin_pairwise<I: InterfaceRole>(
        &mut self,
        place: CommandPlace<'id>,
        command: PairwiseCommand<'id, I>,
        now: Instant,
    ) -> InCommand<'id, PairwiseHeld<'id, I>>;

    /// Poll a pairwise key command.
    fn poll_pairwise<I: InterfaceRole>(
        &mut self,
        command: InCommand<'id, PairwiseHeld<'id, I>>,
        now: Instant,
        sink: &mut impl PortSink,
    ) -> CommandPoll<'id, PairwiseHeld<'id, I>, PairwiseEnd<'id, I>>;

    /// Begin a group key command.
    fn begin_group<I: InterfaceRole>(
        &mut self,
        place: CommandPlace<'id>,
        command: GroupCommand<'id, I>,
        now: Instant,
    ) -> InCommand<'id, GroupHeld<'id, I>>;

    /// Poll a group key command.
    fn poll_group<I: InterfaceRole>(
        &mut self,
        command: InCommand<'id, GroupHeld<'id, I>>,
        now: Instant,
        sink: &mut impl PortSink,
    ) -> CommandPoll<'id, GroupHeld<'id, I>, GroupEnd<'id, I>>;

    /// Begin an agreement command.
    fn begin_agreement<I: InterfaceRole>(
        &mut self,
        place: CommandPlace<'id>,
        command: AgreementCommand<'id, I, Self::Caps>,
        now: Instant,
    ) -> InCommand<'id, AgreementHeld<'id, I>>;

    /// Poll an agreement command.
    fn poll_agreement<I: InterfaceRole>(
        &mut self,
        command: InCommand<'id, AgreementHeld<'id, I>>,
        now: Instant,
        sink: &mut impl PortSink,
    ) -> CommandPoll<'id, AgreementHeld<'id, I>, AgreementEnd<'id, I>>;

    /// Begin a command on an interface's receive rule.
    fn begin_receive<I: InterfaceRole>(
        &mut self,
        place: CommandPlace<'id>,
        command: ReceiveCommand<'id, I, Self::Caps>,
        now: Instant,
    ) -> InCommand<'id, ReceiveHeld<'id, I>>;

    /// Poll a receive command.
    fn poll_receive<I: InterfaceRole>(
        &mut self,
        command: InCommand<'id, ReceiveHeld<'id, I>>,
        now: Instant,
        sink: &mut impl PortSink,
    ) -> CommandPoll<'id, ReceiveHeld<'id, I>, ReceiveEnd<'id, I>>;

    /// Begin a command of values.
    fn begin_value(
        &mut self,
        place: CommandPlace<'id>,
        command: ValueCommand<Self::Caps>,
        now: Instant,
    ) -> InCommand<'id, ValueKind>;

    /// Poll a command of values.
    fn poll_value(
        &mut self,
        command: InCommand<'id, ValueKind>,
        now: Instant,
        sink: &mut impl PortSink,
    ) -> CommandPoll<'id, ValueKind, ValueEnd>;

    /// Open the closed gate.
    fn open_gate(&mut self, gate: GateClosed<'id>) -> GateOpen<'id>;
}

/// The seal of a port's primitives: only this crate makes one, so a core
/// reaches them only through the contract's values that call them.
#[derive(Debug)]
pub struct Sealed(());

impl Sealed {
    pub(crate) const fn new() -> Self {
        Self(())
    }
}

impl<'id> PortKey<'id> {
    /// A command began: the place and what it holds are the port's.
    pub fn began<H>(&self, place: CommandPlace<'id>, held: H) -> InCommand<'id, H> {
        let CommandPlace { brand } = place;
        InCommand { held, brand }
    }

    /// What the command in progress holds, read.
    pub const fn held<'c, H>(&self, command: &'c InCommand<'id, H>) -> &'c H {
        &command.held
    }

    /// A command ended: its place back, and what it held, which its end
    /// returns.
    pub fn command_ended<H>(&self, command: InCommand<'id, H>) -> (CommandPlace<'id>, H) {
        let InCommand { held, brand } = command;
        (CommandPlace { brand }, held)
    }

    /// The pairwise key installed.
    pub const fn pairwise_installed<I>(
        &self,
        slot: PairwiseSlot<'id, I>,
    ) -> InstalledPairwise<'id, I> {
        let PairwiseSlot {
            index,
            brand,
            interface,
        } = slot;
        InstalledPairwise {
            index,
            brand,
            interface,
        }
    }

    /// The pairwise key removed.
    pub const fn pairwise_removed<I>(
        &self,
        key: InstalledPairwise<'id, I>,
    ) -> PairwiseSlot<'id, I> {
        let InstalledPairwise {
            index,
            brand,
            interface,
        } = key;
        PairwiseSlot {
            index,
            brand,
            interface,
        }
    }

    /// A group key installed.
    pub const fn group_installed<I>(&self, slot: GroupSlot<'id, I>) -> InstalledGroup<'id, I> {
        let GroupSlot {
            index,
            brand,
            interface,
        } = slot;
        InstalledGroup {
            index,
            brand,
            interface,
        }
    }

    /// A group key removed, or a change of it that expired.
    pub const fn group_removed<I>(&self, key: InstalledGroup<'id, I>) -> GroupSlot<'id, I> {
        let InstalledGroup {
            index,
            brand,
            interface,
        } = key;
        GroupSlot {
            index,
            brand,
            interface,
        }
    }

    /// An agreement added.
    pub const fn agreement_added<I>(&self, slot: AgreementSlot<'id, I>) -> Agreement<'id, I> {
        let AgreementSlot {
            index,
            brand,
            interface,
        } = slot;
        Agreement {
            index,
            brand,
            interface,
        }
    }

    /// The receive rule set.
    pub const fn receive_set<I>(&self, off: ReceiveOff<'id, I>) -> Receiving<'id, I> {
        let ReceiveOff { brand, interface } = off;
        Receiving { brand, interface }
    }

    /// The receive rule off: an end, or a setting or a change that expired.
    pub const fn receive_ended<I>(&self, receiving: Receiving<'id, I>) -> ReceiveOff<'id, I> {
        let Receiving { brand, interface } = receiving;
        ReceiveOff { brand, interface }
    }

    /// An agreement ended.
    pub const fn agreement_ended<I>(&self, agreement: Agreement<'id, I>) -> AgreementSlot<'id, I> {
        let Agreement {
            index,
            brand,
            interface,
        } = agreement;
        AgreementSlot {
            index,
            brand,
            interface,
        }
    }
}

#[cfg(test)]
mod tests {
    /// A command in progress holds its kind and the rights its end
    /// returns, no value: each token is at most a byte or two.
    #[test]
    fn a_command_in_progress_holds_its_kind_and_rights_alone() {
        use core::mem::size_of;
        let sizes = [
            size_of::<InCommand<'static, GateOpen<'static>>>(),
            size_of::<InCommand<'static, PairwiseHeld<'static, crate::Station>>>(),
            size_of::<InCommand<'static, GroupHeld<'static, crate::Station>>>(),
            size_of::<InCommand<'static, AgreementHeld<'static, crate::Station>>>(),
            size_of::<InCommand<'static, ValueKind>>(),
        ];
        std::println!("tokens: close, pairwise, group, agreement, value: {sizes:?}");
        assert!(sizes.iter().all(|size| *size <= 2));
        assert!(size_of::<PairwiseHeld<'static, crate::Station>>() < size_of::<Ccmp128Key>());
    }

    use super::*;
    extern crate std;

    type StationOf = InterfaceLayout<crate::Station, 1, 2, 2>;
    type AccessPointOf = InterfaceLayout<crate::AccessPoint, 2, 2, 0>;
    type Both = RadioLayout<(First, Second), 16, (StationOf, AccessPointOf)>;
    type One = RadioLayout<(First,), 16, (StationOf,)>;

    /// A layout's mint gives the radio's rights once, a long chain a marker
    /// and four queues, and each interface's bundle.
    #[test]
    fn a_layout_gives_the_radios_rights_and_each_interfaces() {
        let (radio, (station, access_point)) =
            <Both as Layout>::mint::<'static>(crate::MintSeal(()));
        let (_first, _second): (
            LongChain<'static, First, 16>,
            LongChain<'static, Second, 16>,
        ) = radio.chains;
        assert_eq!(radio.queues.map(|queue| queue.category()), QUEUES);
        assert_eq!(station.pairwise.map(|slot| slot.index()), [0]);
        assert_eq!(access_point.pairwise.map(|slot| slot.index()), [0, 1]);
        assert_eq!(access_point.groups.map(|slot| slot.index()), [0, 1]);
    }

    /// A storage reaches each chain's own buffer by the chain's type: each
    /// long chain its marker's, each chain of one its queue's.
    #[test]
    fn each_chain_reaches_its_own_buffer() {
        let (radio, _) = <Both as Layout>::mint::<'static>(crate::MintSeal(()));
        let mut count = 0_u8;
        let mut buffers = Buffers::<u8, (First, Second)>::new(|| {
            count += 1;
            count
        });
        let (first, second) = radio.chains;
        let [voice, video, best_effort, background] = radio.queues;
        let reached = [
            *ChainOf::<Both>::buffer(&first, &mut buffers),
            *ChainOf::<Both>::buffer(&second, &mut buffers),
            *ChainOf::<Both>::buffer(&voice, &mut buffers),
            *ChainOf::<Both>::buffer(&video, &mut buffers),
            *ChainOf::<Both>::buffer(&best_effort, &mut buffers),
            *ChainOf::<Both>::buffer(&background, &mut buffers),
        ];
        let mut sorted = reached;
        sorted.sort_unstable();
        assert_eq!(sorted, [1, 2, 3, 4, 5, 6], "six chains, six buffers");
        assert_eq!(
            [
                *ChainOf::<Both>::buffer_ref(&first, &buffers),
                *ChainOf::<Both>::buffer_ref(&second, &buffers)
            ],
            [1, 2],
            "the long chains' buffers in their markers' order"
        );
        assert_eq!(
            buffers.longs().copied().collect::<std::vec::Vec<_>>(),
            [1, 2]
        );
        assert_eq!(buffers.each().count(), 6);
        for buffer in buffers.each_mut() {
            *buffer = 0;
        }
        assert_eq!(*ChainOf::<Both>::buffer_ref(&voice, &buffers), 0);
        // A set of one long chain has one buffer for it.
        let mut one = Buffers::<u8, (First,)>::new(|| 7);
        let (radio, _) = <One as Layout>::mint::<'static>(crate::MintSeal(()));
        let (first,) = radio.chains;
        *ChainOf::<One>::buffer(&first, &mut one) = 9;
        assert_eq!(one.longs().copied().collect::<std::vec::Vec<_>>(), [9]);
        assert_eq!(one.each().count(), 5);
        assert_eq!(Buffers::<u8, ()>::new(|| 0).each().count(), 4);
    }

    /// The radio's rights and places print what they are: a long chain
    /// its marker and descriptors, a place its state.
    #[test]
    fn the_radios_rights_print_what_they_are() {
        let (radio, _) = <One as Layout>::mint::<'static>(crate::MintSeal(()));
        let printed = std::format!("{radio:?}");
        assert!(printed.contains("RadioRights") && printed.contains("First"));
        assert!(printed.contains("slots: 16") && printed.contains("BestEffort"));
        let RadioRights {
            chains: (first,),
            queues,
            ..
        } = radio;
        let place = LongPlace::new(first);
        let printed = std::format!("{place:?}");
        assert!(printed.contains("LongPlace") && printed.contains("First"));
        assert!(printed.contains("Idle"));
        let places = QueuePlaces::<'static, ()>::new(queues);
        assert!(std::format!("{places:?}").contains("Voice"));
    }

    /// The places of a new installation are free and idle, each of its
    /// category, and give back the queues in their order and the long
    /// chain.
    #[test]
    fn new_places_are_free_and_give_their_rights_back() {
        let (radio, _) = <Both as Layout>::mint::<'static>(crate::MintSeal(()));
        let RadioRights {
            chains: (first, _second),
            queues,
            ..
        } = radio;
        let mut places = QueuePlaces::<'static, u8>::new(queues);
        for category in QUEUES {
            assert_eq!(places.at(category).category(), category);
            assert_eq!(places.at(category).now(), QueueIs::Free);
            assert_eq!(places.at(category).one(), None);
            assert!(matches!(
                places.at_mut(category).state(),
                QueueNow::Free(free) if free.category() == category
            ));
        }
        assert_eq!(places.each().count(), 4);
        let Ok(queues) = places.into_queues() else {
            panic!("free places give their queues back");
        };
        assert_eq!(queues.map(|queue| queue.category()), QUEUES);
        let mut place = LongPlace::new(first);
        assert_eq!(place.now(), LongIs::Idle);
        assert!(matches!(place.state(), LongNow::Idle(_)));
        let Ok(_first): Result<LongChain<'static, First, 16>, _> = place.into_idle() else {
            panic!("an idle place gives its chain back");
        };
    }

    #[test]
    fn a_frame_set_keeps_its_positions_through_its_operations() {
        // A prefix of every count, the whole set at the width and past it.
        assert_eq!(FrameSet::prefix(0), FrameSet::EMPTY);
        assert_eq!(FrameSet::prefix(3).bits(), 0b111);
        assert_eq!(FrameSet::prefix(64).bits(), u64::MAX);
        assert_eq!(FrameSet::prefix(200).bits(), u64::MAX);
        let set = FrameSet::from_bits(0b1010);
        assert!(set.contains(1) && set.contains(3));
        assert!(!set.contains(0) && !set.contains(2));
        // A position past the width is in no set.
        assert!(!FrameSet::prefix(64).contains(64));
        assert_eq!(set.len(), 2);
        assert!(!set.is_empty() && FrameSet::EMPTY.is_empty());
        let other = FrameSet::from_bits(0b0110);
        assert_eq!(set.union(other).bits(), 0b1110);
        assert_eq!(set.without(other).bits(), 0b1000);
    }

    /// A set's count is its frames, up to the whole 64.
    #[test]
    fn a_frame_set_counts_its_frames() {
        assert_eq!(super::FrameSet::from_bits(0).count(), 0);
        assert_eq!(super::FrameSet::from_bits(0b1011).count(), 3);
        assert_eq!(super::FrameSet::from_bits(u64::MAX).count(), 64);
    }

    /// A front's length is taken up to the headroom, and no further.
    #[test]
    fn a_front_is_as_long_as_the_headroom_at_most() {
        use super::{HEADROOM, HeaderLen};
        assert_eq!(HeaderLen::new(0).map(HeaderLen::get), Some(0));
        assert_eq!(HeaderLen::new(HEADROOM).map(HeaderLen::get), Some(HEADROOM));
        assert_eq!(HeaderLen::new(HEADROOM + 1), None);
        assert_eq!(HeaderLen::new(usize::MAX), None);
    }
}
