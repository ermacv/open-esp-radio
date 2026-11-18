//! The data path (`design/port-contract-2026-11-16.md`, section 4):
//! aggregates as places filled and published once, singles a place a
//! purpose, commands, and the facts that wake their caller.
//!
//! No call returns an error. A call that needs a state takes its right,
//! each a value of at most two words branded with the installation, built
//! by this crate alone: the port makes the next right with its
//! [`PortKey`](crate::life::PortKey).

use core::marker::PhantomData;
use core::task::Context;

use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_wire::address::IndividualAddress;
use oer_protocols_ieee80211_wire::ccmp::GroupKeyId;
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;
use oer_protocols_ieee80211_wire::security::Spp;
use oer_protocols_radio_port::ClockError;

use crate::life::{MintData, PortKey, RadioPort};
use crate::{
    AggregateOutcome, AmpduRate, BssColor, GateClosed, GateOpen, Ieee80211Instant, MpduLength,
    MpduStartSpacing, PortRate, Protection, RetryPlan, SingleOutcome, StationHe, StationReceive,
    SupportedBlockAck, SupportedEdca, TbttEvent, Tid, TxFrame,
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

/// One of the two arenas.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ArenaSide {
    /// The first.
    First,
    /// The second.
    Second,
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
        self.0.count_ones() as u8
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

right!(
    /// An arena that holds nothing: what an installation gives and a
    /// settlement or a withdrawal returns.
    IdleArena { side: ArenaSide }
);
right!(
    /// An arena opened for a link and a TID, being filled.
    Arena { side: ArenaSide, frames: u8 }
);
right!(
    /// An arena on the air: the frames of its prefix that went.
    OnAir { side: ArenaSide, sent: u8 }
);
right!(
    /// A single's place: one of a purpose of a role, a const count of the
    /// installation.
    SinglePlace { index: u8 }
);
right!(
    /// The right to publish on a transmit queue: it says the queue is free.
    QueueRight { category: WmmAccessCategory }
);
right!(
    /// The one place of a command.
    CommandPlace {}
);
right!(
    /// The station's pairwise key slot, empty.
    PairwiseSlot {}
);
right!(
    /// The station's pairwise key, installed.
    InstalledPairwise {}
);
right!(
    /// One of the station's group key slots, empty.
    GroupSlot { index: u8 }
);
right!(
    /// One of the station's group keys, installed.
    InstalledGroup { index: u8 }
);
right!(
    /// A receive Block Ack agreement's slot, empty.
    AgreementSlot { index: u8 }
);
right!(
    /// A receive Block Ack agreement, running.
    Agreement { index: u8 }
);

impl IdleArena<'_> {
    /// The arena.
    pub const fn side(&self) -> ArenaSide {
        self.side
    }
}

impl Arena<'_> {
    /// The arena.
    pub const fn side(&self) -> ArenaSide {
        self.side
    }

    /// The frames it holds.
    pub const fn frames(&self) -> u8 {
        self.frames
    }
}

impl OnAir<'_> {
    /// The arena.
    pub const fn side(&self) -> ArenaSide {
        self.side
    }

    /// The frames of the prefix that went; the others stay in their
    /// places with no attempt spent.
    pub const fn sent(&self) -> FrameSet {
        FrameSet::prefix(self.sent)
    }

    /// How many went: what went is always a prefix, so the set is derived.
    pub const fn went(&self) -> u8 {
        self.sent
    }
}

impl SinglePlace<'_> {
    /// The place's index: the purpose it is of.
    pub const fn index(&self) -> u8 {
        self.index
    }
}

impl QueueRight<'_> {
    /// The queue.
    pub const fn category(&self) -> WmmAccessCategory {
        self.category
    }
}

macro_rules! indexed {
    ($($name:ident),*) => {$(
        impl $name<'_> {
            /// The slot's index.
            pub const fn index(&self) -> u8 {
                self.index
            }
        }
    )*};
}
indexed!(GroupSlot, InstalledGroup, AgreementSlot, Agreement);

/// The access categories, a queue each, in the order of
/// [`DataRights::queues`].
pub const QUEUES: [WmmAccessCategory; 4] = [
    WmmAccessCategory::Voice,
    WmmAccessCategory::Video,
    WmmAccessCategory::BestEffort,
    WmmAccessCategory::Background,
];

/// The data's rights of an installation: two arenas, `SINGLES` single
/// places of its controller, `DATA` single places of an access point's
/// data, a right a transmit queue, the command's place, the key slots and
/// the receive agreements' slots, all empty. The machine holds them
/// while the data is stopped, the engine while it flows; a retirement
/// takes them whole.
#[must_use = "the data's rights each have one holder"]
#[derive(Debug)]
pub struct DataRights<
    'id,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const DATA: usize,
> {
    /// The two arenas.
    pub arenas: [IdleArena<'id>; 2],
    /// The controller's single places, one a purpose.
    pub singles: [SinglePlace<'id>; SINGLES],
    /// An access point's data places, one a transmit queue in the order of
    /// [`QUEUES`]; a station's data goes in the arenas and has none
    /// ([`DataLayout::CHECKED`]).
    pub data: [SinglePlace<'id>; DATA],
    /// A right a queue, in the order of [`QUEUES`].
    pub queues: [QueueRight<'id>; 4],
    /// The command's place.
    pub command: CommandPlace<'id>,
    /// The station's pairwise key slot.
    pub pairwise: PairwiseSlot<'id>,
    /// The station's group key slots.
    pub groups: [GroupSlot<'id>; GROUPS],
    /// The receive agreements' slots.
    pub agreements: [AgreementSlot<'id>; AGREEMENTS],
}

/// The layout of an installation's data's rights.
pub struct DataLayout<
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const DATA: usize,
>;

/// The data places of an access point's layout: one a transmit queue.
pub const DATA_SINGLES: usize = QUEUES.len();

impl<const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize, const DATA: usize>
    DataLayout<SINGLES, GROUPS, AGREEMENTS, DATA>
{
    /// The layout's data places are none (a station's) or one a transmit
    /// queue (an access point's, [`DATA_SINGLES`]): a single of an access
    /// point's data takes its queue's place, so another number is no
    /// layout. Its mint evaluates it; a layout of another number does not
    /// build.
    pub const CHECKED: () = if DATA != 0 && DATA != DATA_SINGLES {
        panic!("a layout has no data place or one a transmit queue")
    };
}

impl<const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize, const DATA: usize>
    crate::life::sealed::Layout for DataLayout<SINGLES, GROUPS, AGREEMENTS, DATA>
{
}

impl<const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize, const DATA: usize> MintData
    for DataLayout<SINGLES, GROUPS, AGREEMENTS, DATA>
{
    type Rights<'id> = DataRights<'id, SINGLES, GROUPS, AGREEMENTS, DATA>;

    fn mint<'id>(seal: crate::life::MintSeal) -> Self::Rights<'id> {
        let crate::life::MintSeal(()) = seal;
        const { Self::CHECKED };
        const {
            if SINGLES + DATA > u8::MAX as usize
                || GROUPS > u8::MAX as usize
                || AGREEMENTS > u8::MAX as usize
            {
                panic!("an installation's places are counted in a byte");
            }
        }
        DataRights {
            arenas: [ArenaSide::First, ArenaSide::Second].map(|side| IdleArena {
                side,
                brand: PhantomData,
            }),
            singles: core::array::from_fn(|index| SinglePlace {
                index: index as u8,
                brand: PhantomData,
            }),
            // After the controller's: each place's index is the installation's
            // own. The sum fits a byte (the const check above), so the
            // addition saturates nowhere.
            data: core::array::from_fn(|index| SinglePlace {
                index: SINGLES.saturating_add(index) as u8,
                brand: PhantomData,
            }),
            queues: QUEUES.map(|category| QueueRight {
                category,
                brand: PhantomData,
            }),
            command: CommandPlace { brand: PhantomData },
            pairwise: PairwiseSlot { brand: PhantomData },
            groups: core::array::from_fn(|index| GroupSlot {
                index: index as u8,
                brand: PhantomData,
            }),
            agreements: core::array::from_fn(|index| AgreementSlot {
                index: index as u8,
                brand: PhantomData,
            }),
        }
    }
}

/// What a settlement leaves of the arena.
#[derive(Debug)]
pub enum Settled<'id> {
    /// The kept frames stay in their places for the retry.
    Kept(Arena<'id>),
    /// The arena holds nothing.
    Idle(IdleArena<'id>),
}

/// The arena takes no more frames: its slots are full. The one answer of
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
        if len <= HEADROOM {
            Some(Self(len as u8))
        } else {
            None
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

/// How an arena goes on the air.
pub enum Ppdu<'id, K> {
    /// The arena's first frame as one MPDU, acknowledged, by this plan.
    Single(RetryPlan<PortRate<'id, K>>),
    /// The longest prefix the rate admits as an A-MPDU; if that prefix is
    /// one frame, that frame as one MPDU by `alone`.
    Aggregate {
        /// The aggregate's rate.
        rate: AmpduRate<'id, K>,
        /// The recipient's Minimum MPDU Start Spacing, the peer's for the
        /// link's life.
        spacing: MpduStartSpacing,
        /// The control exchange that protects the aggregate, as the upper
        /// MAC's policy decided it this publication.
        protection: Protection,
        /// The plan of a prefix of one frame.
        alone: RetryPlan<PortRate<'id, K>>,
    },
}

impl<K> Clone for Ppdu<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K> Copy for Ppdu<'_, K> {}

/// How an arena's publication ended: as what went, not as what was asked.
/// An [`Ppdu::Aggregate`] whose prefix was one frame ends `Single`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArenaOutcome<A, B, D> {
    /// One MPDU went.
    Single(SingleOutcome<A, D>),
    /// An A-MPDU went.
    Aggregate(AggregateOutcome<B, D>),
}

/// An arena's end, kept by the port and read in place through the
/// reference [`DataPort::aggregate_ended`] returns: what the prefix's frames
/// did on the air.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ended<A, B, D> {
    /// What went, and how it ended.
    pub outcome: ArenaOutcome<A, B, D>,
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

/// A command on the station's pairwise key slot.
#[derive(Debug)]
pub enum PairwiseCommand<'id> {
    /// Install the station's pairwise key in the empty slot.
    Install {
        /// The empty slot.
        slot: PairwiseSlot<'id>,
        /// The peer.
        peer: IndividualAddress,
        /// The key.
        key: Ccmp128Key,
        /// The association's SPP A-MSDU decision.
        spp: Spp,
    },
    /// Remove the pairwise key.
    Remove(InstalledPairwise<'id>),
}

/// What a pairwise key command in progress holds: the slot it installs
/// in, or the key it removes.
#[derive(Debug)]
pub enum PairwiseHeld<'id> {
    /// An installation's empty slot.
    Installing(PairwiseSlot<'id>),
    /// A removal's installed key.
    Removing(InstalledPairwise<'id>),
}

/// A pairwise key command's end: the slot always comes back.
#[derive(Debug)]
pub enum PairwiseEnd<'id> {
    /// The key is installed.
    Installed(InstalledPairwise<'id>),
    /// The slot is empty: an installation that expired (the port cleared
    /// the entry) or a removal.
    Empty(PairwiseSlot<'id>),
}

/// A command on a group key slot.
#[derive(Debug)]
pub enum GroupCommand<'id> {
    /// Install a group key in an empty slot.
    Install {
        /// The empty slot.
        slot: GroupSlot<'id>,
        /// The key's ID.
        key_id: GroupKeyId,
        /// The key.
        key: Ccmp128Key,
    },
    /// Replace an installed group key.
    Replace {
        /// The installed key.
        installed: InstalledGroup<'id>,
        /// The new key's ID.
        key_id: GroupKeyId,
        /// The new key.
        key: Ccmp128Key,
    },
    /// Make an installed group key the one received under its ID.
    Select(InstalledGroup<'id>),
    /// Remove a group key.
    Remove(InstalledGroup<'id>),
}

/// What a group key command in progress holds: its kind and its slot.
#[derive(Debug)]
pub enum GroupHeld<'id> {
    /// An installation's empty slot.
    Installing(GroupSlot<'id>),
    /// A replacement's installed key.
    Replacing(InstalledGroup<'id>),
    /// A selection's installed key.
    Selecting(InstalledGroup<'id>),
    /// A removal's installed key.
    Removing(InstalledGroup<'id>),
}

/// A group key command's end: the slot always comes back.
#[derive(Debug)]
pub enum GroupEnd<'id> {
    /// A group key is installed, replaced or selected.
    Installed(InstalledGroup<'id>),
    /// The slot is empty: an installation, replacement or selection that
    /// expired (the port cleared the entry), or a removal.
    Empty(GroupSlot<'id>),
}

/// A command on a receive Block Ack agreement's slot.
#[derive(Debug)]
pub enum AgreementCommand<'id, K> {
    /// Begin a receive Block Ack agreement.
    Add {
        /// The empty slot.
        slot: AgreementSlot<'id>,
        /// The agreement.
        agreement: SupportedBlockAck<K>,
    },
    /// End a receive Block Ack agreement.
    End(Agreement<'id>),
}

/// What an agreement command in progress holds: the slot it adds in, or
/// the agreement it ends.
#[derive(Debug)]
pub enum AgreementHeld<'id> {
    /// An addition's empty slot.
    Adding(AgreementSlot<'id>),
    /// An end's running agreement.
    Ending(Agreement<'id>),
}

/// An agreement command's end: the slot always comes back.
#[derive(Debug)]
pub enum AgreementEnd<'id> {
    /// An agreement runs.
    Added(Agreement<'id>),
    /// The slot is empty: an addition that expired, or an end.
    Empty(AgreementSlot<'id>),
}

/// A command of values: it carries no right.
pub enum ValueCommand<K> {
    /// Configure the station's receive.
    Configure(StationReceive<K>),
    /// Set the station's EDCA parameters.
    SetEdca(SupportedEdca<K>),
    /// Set the BSS color.
    SetBssColor(BssColor),
    /// Set the station's HE state.
    SetHe(StationHe),
}

/// What a command of values in progress holds: its kind alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueKind {
    /// The station's receive configured.
    Configure,
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

/// What receives the ends a port's service makes.
pub trait PortSink<'id, A, D> {
    /// A single's end, with its place and its queue's right.
    fn single_ended(
        &mut self,
        outcome: SingleOutcome<A, D>,
        place: SinglePlace<'id>,
        queue: QueueRight<'id>,
    );
    /// A TBTT of the station's schedule.
    fn tbtt(&mut self, event: TbttEvent);
}

/// The data path, as a port gives it (section 4).
pub trait DataPort<'id>: RadioPort<'id> {
    /// The link an arena is opened for: it carries its key.
    type Link;
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

    /// Open an idle arena for `link` and `tid`.
    fn open(&mut self, idle: IdleArena<'id>, link: &Self::Link, tid: Tid) -> Arena<'id>;

    /// The next frame's place of `length` in the arena, or [`Complete`]
    /// when its slots are full.
    fn place<'a>(
        &'a mut self,
        arena: &'a mut Arena<'id>,
        length: MpduLength<Self::Caps>,
    ) -> Result<Place<'a, 'id, Self>, Complete>
    where
        Self: Sized;

    /// Write a frame into its place: the body by its handle, in place, and
    /// the header once into the headroom the port lends. The port's
    /// primitive, reached through [`Place::write`].
    fn write_frame(
        &mut self,
        token: Sealed,
        arena: &mut Arena<'id>,
        body: Self::Memory,
        header: impl FnOnce(&mut [u8; HEADROOM]) -> HeaderLen,
    );

    /// Publish the arena as `how` says: a single its first frame; an
    /// aggregate the longest prefix of its frames the rate admits, one
    /// frame at least, which goes alone by its plan. The others stay in
    /// their places with no attempt spent.
    fn publish(
        &mut self,
        arena: Arena<'id>,
        how: Ppdu<'id, Self::Caps>,
        gate: &GateOpen<'id>,
        now: Instant,
    ) -> OnAir<'id>;

    /// Withdraw an arena's frames: each body goes home.
    fn withdraw(
        &mut self,
        arena: Arena<'id>,
        home: &mut impl FnMut(Self::Memory),
    ) -> IdleArena<'id>;

    /// The end of the arena on the air, read in place, once it ended.
    fn aggregate_ended(
        &mut self,
        facts: TxFacts,
        on_air: &OnAir<'id>,
        now: Instant,
    ) -> Option<&Ended<Self::Ack, Self::BlockAck, Self::Denial>>;

    /// Settle an ended arena: the frames of `keep` stay in their places for
    /// a retry, every other body goes home.
    fn settle(
        &mut self,
        on_air: OnAir<'id>,
        keep: FrameSet,
        home: &mut impl FnMut(Self::Memory),
    ) -> Settled<'id>;

    /// Serve the transmit facts: singles' ends, the queues' next, the TBTT.
    fn serve(
        &mut self,
        facts: TxFacts,
        tbtt: Signal,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Ack, Self::Denial>,
    );

    /// Write a single into its place.
    fn write_single(
        &mut self,
        place: &mut SinglePlace<'id>,
        fill: impl FnOnce(&mut [u8]) -> MpduLength<Self::Caps>,
    );

    /// Publish a single written in its place on its free queue. From its
    /// publication to its end the place and the queue's right are the
    /// port's; its end gives them back through the sink
    /// ([`PortSink::single_ended`]).
    fn publish_single(
        &mut self,
        place: SinglePlace<'id>,
        queue: QueueRight<'id>,
        gate: &GateOpen<'id>,
        frame: &TxFrame<'id, Self::Caps>,
        now: Instant,
    );

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
        sink: &mut impl PortSink<'id, Self::Ack, Self::Denial>,
    ) -> CommandPoll<'id, GateOpen<'id>, GateClosed<'id>>;

    /// Begin a pairwise key command.
    fn begin_pairwise(
        &mut self,
        place: CommandPlace<'id>,
        command: PairwiseCommand<'id>,
        now: Instant,
    ) -> InCommand<'id, PairwiseHeld<'id>>;

    /// Poll a pairwise key command.
    fn poll_pairwise(
        &mut self,
        command: InCommand<'id, PairwiseHeld<'id>>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Ack, Self::Denial>,
    ) -> CommandPoll<'id, PairwiseHeld<'id>, PairwiseEnd<'id>>;

    /// Begin a group key command.
    fn begin_group(
        &mut self,
        place: CommandPlace<'id>,
        command: GroupCommand<'id>,
        now: Instant,
    ) -> InCommand<'id, GroupHeld<'id>>;

    /// Poll a group key command.
    fn poll_group(
        &mut self,
        command: InCommand<'id, GroupHeld<'id>>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Ack, Self::Denial>,
    ) -> CommandPoll<'id, GroupHeld<'id>, GroupEnd<'id>>;

    /// Begin an agreement command.
    fn begin_agreement(
        &mut self,
        place: CommandPlace<'id>,
        command: AgreementCommand<'id, Self::Caps>,
        now: Instant,
    ) -> InCommand<'id, AgreementHeld<'id>>;

    /// Poll an agreement command.
    fn poll_agreement(
        &mut self,
        command: InCommand<'id, AgreementHeld<'id>>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Ack, Self::Denial>,
    ) -> CommandPoll<'id, AgreementHeld<'id>, AgreementEnd<'id>>;

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
        sink: &mut impl PortSink<'id, Self::Ack, Self::Denial>,
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

/// A frame's place in an arena, borrowed for one write.
#[must_use = "a place is written once"]
pub struct Place<'a, 'id, P: DataPort<'id>> {
    port: &'a mut P,
    arena: &'a mut Arena<'id>,
}

/// The port's word that it found a free place in an arena, made by its
/// key: what a [`Place`] is made from.
#[must_use = "a place is made from it"]
#[derive(Debug)]
pub struct Placing<'id> {
    brand: Brand<'id>,
}

impl<'a, 'id, P: DataPort<'id>> Place<'a, 'id, P> {
    /// The port's place in `arena`, which it found free.
    pub fn new(placing: Placing<'id>, port: &'a mut P, arena: &'a mut Arena<'id>) -> Self {
        let Placing { brand: _ } = placing;
        Self { port, arena }
    }

    /// Write the frame: the body by its handle, in place, the header once
    /// into the headroom the port lends.
    pub fn write(self, body: P::Memory, header: impl FnOnce(&mut [u8; HEADROOM]) -> HeaderLen) {
        let Self { port, arena } = self;
        port.write_frame(Sealed::new(), arena, body, header);
        arena.frames = arena.frames.saturating_add(1);
    }
}

impl<'id> PortKey<'id> {
    /// An arena opened.
    pub const fn open(&self, idle: IdleArena<'id>) -> Arena<'id> {
        let IdleArena { side, brand } = idle;
        Arena {
            side,
            frames: 0,
            brand,
        }
    }

    /// An arena published: its first `sent` frames went.
    pub const fn publish(&self, arena: Arena<'id>, sent: u8) -> OnAir<'id> {
        let Arena { side, brand, .. } = arena;
        OnAir { side, sent, brand }
    }

    /// An arena withdrawn.
    pub const fn withdraw(&self, arena: Arena<'id>) -> IdleArena<'id> {
        let Arena { side, brand, .. } = arena;
        IdleArena { side, brand }
    }

    /// An arena settled with `kept` frames left in their places.
    pub const fn settle(&self, on_air: OnAir<'id>, kept: u8) -> Settled<'id> {
        let OnAir { side, brand, .. } = on_air;
        if kept == 0 {
            Settled::Idle(IdleArena { side, brand })
        } else {
            Settled::Kept(Arena {
                side,
                frames: kept,
                brand,
            })
        }
    }

    /// The port found a free place in an arena.
    pub const fn placing(&self) -> Placing<'id> {
        Placing { brand: PhantomData }
    }

    /// A single published: its place and its queue's right are the
    /// port's until its end, which [`Self::single_end`] gives back.
    pub const fn single_published(&self, place: SinglePlace<'id>, queue: QueueRight<'id>) {
        let SinglePlace { .. } = place;
        let QueueRight { .. } = queue;
    }

    /// A single ended: the place and the queue's right of the single the
    /// port published at `index` on `category`, which it held since its
    /// publication.
    pub const fn single_end(
        &self,
        index: u8,
        category: WmmAccessCategory,
    ) -> (SinglePlace<'id>, QueueRight<'id>) {
        (
            SinglePlace {
                index,
                brand: PhantomData,
            },
            QueueRight {
                category,
                brand: PhantomData,
            },
        )
    }

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
    pub const fn pairwise_installed(&self, slot: PairwiseSlot<'id>) -> InstalledPairwise<'id> {
        let PairwiseSlot { brand } = slot;
        InstalledPairwise { brand }
    }

    /// The pairwise key removed.
    pub const fn pairwise_removed(&self, key: InstalledPairwise<'id>) -> PairwiseSlot<'id> {
        let InstalledPairwise { brand } = key;
        PairwiseSlot { brand }
    }

    /// A group key installed.
    pub const fn group_installed(&self, slot: GroupSlot<'id>) -> InstalledGroup<'id> {
        let GroupSlot { index, brand } = slot;
        InstalledGroup { index, brand }
    }

    /// A group key removed, or a change of it that expired.
    pub const fn group_removed(&self, key: InstalledGroup<'id>) -> GroupSlot<'id> {
        let InstalledGroup { index, brand } = key;
        GroupSlot { index, brand }
    }

    /// An agreement added.
    pub const fn agreement_added(&self, slot: AgreementSlot<'id>) -> Agreement<'id> {
        let AgreementSlot { index, brand } = slot;
        Agreement { index, brand }
    }

    /// An agreement ended.
    pub const fn agreement_ended(&self, agreement: Agreement<'id>) -> AgreementSlot<'id> {
        let Agreement { index, brand } = agreement;
        AgreementSlot { index, brand }
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
            size_of::<InCommand<'static, PairwiseHeld<'static>>>(),
            size_of::<InCommand<'static, GroupHeld<'static>>>(),
            size_of::<InCommand<'static, AgreementHeld<'static>>>(),
            size_of::<InCommand<'static, ValueKind>>(),
        ];
        std::println!("tokens: close, pairwise, group, agreement, value: {sizes:?}");
        assert!(sizes.iter().all(|size| *size <= 2));
        assert!(size_of::<PairwiseHeld<'static>>() < size_of::<Ccmp128Key>());
    }

    use super::*;
    extern crate std;

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
}
