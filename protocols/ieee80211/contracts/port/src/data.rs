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
use oer_protocols_radio_port::{ClockError, PoolPath, Root};

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
    OnAir { side: ArenaSide, sent: FrameSet }
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
    /// A command in progress.
    InCommand {}
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
/// places, a right a transmit queue, the command's place, the key slots
/// and the receive agreements' slots, all empty. The machine holds them
/// while the data is stopped, the engine while it flows; a retirement
/// takes them whole.
#[must_use = "the data's rights each have one holder"]
#[derive(Debug)]
pub struct DataRights<'id, const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize> {
    /// The two arenas.
    pub arenas: [IdleArena<'id>; 2],
    /// The single places, one a purpose.
    pub singles: [SinglePlace<'id>; SINGLES],
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
pub struct DataLayout<const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize>;

impl<const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize> MintData
    for DataLayout<SINGLES, GROUPS, AGREEMENTS>
{
    type Rights<'id> = DataRights<'id, SINGLES, GROUPS, AGREEMENTS>;

    fn mint<'id>() -> Self::Rights<'id> {
        const {
            if SINGLES > u8::MAX as usize
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

/// An arena's end, read in place: what the prefix's frames did on the air.
#[derive(Debug)]
pub struct Ended<'a, A, B, D> {
    /// What went, and how it ended.
    pub outcome: &'a ArenaOutcome<A, B, D>,
    /// The frames of the prefix that went.
    pub sent: FrameSet,
    /// The frames the response acknowledged.
    pub acknowledged: FrameSet,
}

/// A command the port runs behind its barrier. It carries the right it
/// needs; its end ([`CommandEnd`]) returns it.
pub enum Command<'id, K> {
    /// Install the station's pairwise key.
    InstallPairwise {
        /// The empty slot.
        slot: PairwiseSlot<'id>,
        /// The peer.
        peer: IndividualAddress,
        /// The key.
        key: Ccmp128Key,
        /// The association's SPP A-MSDU decision.
        spp: Spp,
    },
    /// Install a group key in an empty slot.
    InstallGroup {
        /// The empty slot.
        slot: GroupSlot<'id>,
        /// The key's ID.
        key_id: GroupKeyId,
        /// The key.
        key: Ccmp128Key,
    },
    /// Replace an installed group key.
    ReplaceGroup {
        /// The installed key.
        installed: InstalledGroup<'id>,
        /// The new key's ID.
        key_id: GroupKeyId,
        /// The new key.
        key: Ccmp128Key,
    },
    /// Make an installed group key the one received under its ID.
    SelectGroup(InstalledGroup<'id>),
    /// Remove the pairwise key.
    RemovePairwise(InstalledPairwise<'id>),
    /// Remove a group key.
    RemoveGroup(InstalledGroup<'id>),
    /// Begin a receive Block Ack agreement.
    AddAgreement {
        /// The empty slot.
        slot: AgreementSlot<'id>,
        /// The agreement.
        agreement: SupportedBlockAck<K>,
    },
    /// End a receive Block Ack agreement.
    EndAgreement(Agreement<'id>),
    /// Configure the station's receive.
    Configure(StationReceive<K>),
    /// Close the gate, once what is on the air ended.
    CloseGate(GateOpen<'id>),
    /// Set the station's EDCA parameters.
    SetEdca(SupportedEdca<K>),
    /// Set the BSS color.
    SetBssColor(BssColor),
    /// Set the station's HE state.
    SetHe(StationHe),
}

/// A command's end: one sum, a variant a kind of end, each carrying what
/// that end returns.
#[derive(Debug)]
pub enum CommandEnd<'id, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    /// A command of values took effect.
    Done,
    /// A command of values did not take effect within its bound.
    Expired,
    /// The gate is closed. It has no expired end.
    GateClosed(GateClosed<'id, Path>),
    /// The pairwise key is installed.
    PairwiseInstalled(InstalledPairwise<'id>),
    /// The pairwise slot is empty: an installation that expired (the port
    /// cleared the entry) or a removal.
    PairwiseEmpty(PairwiseSlot<'id>),
    /// A group key is installed, replaced or selected.
    GroupInstalled(InstalledGroup<'id>),
    /// A group slot is empty: an installation, replacement or selection
    /// that expired (the port cleared the entry), or a removal.
    GroupEmpty(GroupSlot<'id>),
    /// An agreement runs.
    AgreementAdded(Agreement<'id>),
    /// An agreement's slot is empty: an addition that expired or an end.
    AgreementEmpty(AgreementSlot<'id>),
}

/// A command's poll.
#[derive(Debug)]
pub enum CommandPoll<'id> {
    /// In progress; the port's facts wake.
    Pending(InCommand<'id>),
    /// It ended, with the command's place.
    Ended(CommandPlace<'id>, CommandEnd<'id>),
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
    ) -> Option<Ended<'_, Self::Ack, Self::BlockAck, Self::Denial>>;

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

    /// Begin a command.
    fn begin(
        &mut self,
        place: CommandPlace<'id>,
        command: Command<'id, Self::Caps>,
        now: Instant,
    ) -> InCommand<'id>;

    /// Poll a command.
    fn poll_command(
        &mut self,
        command: InCommand<'id>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Ack, Self::Denial>,
    ) -> CommandPoll<'id>;

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

    /// An arena published: its prefix `sent` went.
    pub const fn publish(&self, arena: Arena<'id>, sent: FrameSet) -> OnAir<'id> {
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

    /// A command began.
    pub const fn begin(&self, place: CommandPlace<'id>) -> InCommand<'id> {
        let CommandPlace { brand } = place;
        InCommand { brand }
    }

    /// A command ended: its place back.
    pub const fn command_ended(&self, command: InCommand<'id>) -> CommandPlace<'id> {
        let InCommand { brand } = command;
        CommandPlace { brand }
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
