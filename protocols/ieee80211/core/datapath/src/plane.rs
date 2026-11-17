//! The connected station's data plane over the port's two transmit arenas.
//!
//! The plane holds what one link's data needs ([`DataLink`], handed over at
//! connect and returned at stop): the header template, the access
//! categories the link admits, each TID's operational transmit Block Ack
//! window, the retry policy and the access point's HT limits. It also holds
//! the right to each arena, in its place beside a book: what the plane
//! knows of the frames the arena holds (their TID, first sequence number,
//! octets, each MSDU's admission and the attempts each went in).
//!
//! The sequence spaces and the pairwise key's packet numbers are the link's
//! ([`TxNumbers`]), which its holder numbers the station's own frames from
//! too (EAPOL, Block Ack negotiation): each intake draws from them.
//!
//! An MSDU becomes an MPDU once, in place, as the arena takes it
//! ([`DataPlane::intake`]): its body stays in the network's memory and the
//! plane writes the MPDU's front (its header, the CCMP header, LLC/SNAP)
//! into the headroom the port lends, flush against the body, reading the
//! Ethernet header the network left there; its sequence number and packet
//! number are drawn there. The arena that is filled (the standby) belongs
//! to one TID at a time and holds consecutive sequence numbers inside that
//! TID's window, up to the TID's [`PpduLimit`] (one addition and one
//! comparison per frame) and the arena's slots; a TID without an
//! operational agreement takes one frame. An MSDU the standby cannot take
//! now waits in the network's queue ([`Intake::Wait`]), its memory
//! untouched.
//!
//! [`DataPlane::publication`] puts one arena on the air at a time, as one
//! plan gives it: an arena holding frames that wait from an earlier
//! publication first, then the standby. More frames than one go as an
//! A-MPDU under the TID's agreement, the longest prefix the rate admits; one
//! frame, or the frames of a TID without agreement, go as single MPDUs, the
//! arena's first frame each time. [`DataPlane::completed`] reads the
//! arena's end once, in section 4's order: the end, the standby published
//! at once when a BlockAck acknowledged every frame that went, then the
//! settlement. A frame acknowledged is delivered; a missing one is dropped
//! once its MSDU's lifetime or the policy's attempts ran out, and kept in
//! its place otherwise with the same sequence and packet numbers: the arena
//! goes again before the standby. A frame that did not go stays in its
//! place with no attempt spent. A frame dropped under an agreement moves
//! the TID's window, and the recipient's is owed a BlockAckReq
//! ([`Settled::bar`]). The end also teaches the rate selection
//! ([`RateLesson`]) and charges what it cost ([`crate::cost`]).
//!
//! The plane reads no clock: every instant is the caller's.

use super::{AggregateRetry, DataProtection, ETHERNET_HEADER_LEN, PpduLimit, aggregate};
use crate::cost::{aggregate_cost, single_cost};
use crate::events::{AggregateSettled, DataEvent, DataLoss};
use crate::numbers::{TxKey, TxNumbers};
use oer_portable_events::{EventSink, report};
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    AggregateEnd, AggregateOutcome, Arena, ArenaOutcome, ArenaSide, DataPort, FrameSet, GateOpen,
    HEADROOM, HeaderLen, IdleArena, Ieee80211Capabilities, MpduLength, MpduStartSpacing, OnAir,
    PhyRate, Ppdu, RxMetadata, Settled as Kept, SingleEnd, SingleOutcome, SingleResponse, Tid,
    TxEstimate, TxFacts, TxOutcome,
};
use oer_protocols_ieee80211_core_upper_mac::{
    qos::AcAdmission,
    rate_control::BlockAckTally,
    rate_selection::{FrameRates, RateSelection},
    sequence::SequenceSpace,
};
use oer_protocols_ieee80211_wire::qos::WmmUserPriority;
use oer_protocols_ieee80211_wire::{
    ccmp::{CCMP_HEADER_LEN, CCMP_MIC_LEN, CcmpHeader, CcmpKeyId},
    data::PeerQos,
    data::{IEEE80211_LEGACY_DATA_HEADER_LEN, IEEE80211_QOS_DATA_HEADER_LEN, LLC_SNAP_HEADER_LEN},
    security::LinkProtection,
    sequence::SequenceNumber,
    station::{StaDataHeaderFields, StaDataHeaderTemplate},
};

/// The frame positions of one arena book: the width of its frame sets.
const BOOK_FRAMES: usize = 64;

/// The FCS every MPDU carries on the air (IEEE Std 802.11-2020, 9.2.4.8).
const FCS_LEN: usize = 4;

/// The longest front the station writes fits the headroom the port lends.
const _: () = if StaDataHeaderTemplate::MAX_PREFIX_LEN > HEADROOM {
    panic!("the station's longest data front exceeds the port's headroom")
};

/// The front's length of each of the template's four kinds, checked once.
const fn front_len<const LEN: usize>() -> HeaderLen {
    const {
        match HeaderLen::new(LEN) {
            Some(len) => len,
            None => panic!("a data front fits the headroom"),
        }
    }
}

/// The length of the front `header` writes, as the port reads it.
const fn front_of(header: &StaDataHeaderTemplate) -> HeaderLen {
    const LEGACY_OPEN: usize = IEEE80211_LEGACY_DATA_HEADER_LEN + LLC_SNAP_HEADER_LEN;
    const QOS_OPEN: usize = IEEE80211_QOS_DATA_HEADER_LEN + LLC_SNAP_HEADER_LEN;
    const LEGACY_CCMP: usize =
        IEEE80211_LEGACY_DATA_HEADER_LEN + CCMP_HEADER_LEN + LLC_SNAP_HEADER_LEN;
    const QOS_CCMP: usize = IEEE80211_QOS_DATA_HEADER_LEN + CCMP_HEADER_LEN + LLC_SNAP_HEADER_LEN;
    match (header.peer_qos(), header.protection()) {
        (PeerQos::NonQos, LinkProtection::Open) => front_len::<LEGACY_OPEN>(),
        (PeerQos::Qos, LinkProtection::Open) => front_len::<QOS_OPEN>(),
        (PeerQos::NonQos, LinkProtection::Ccmp) => front_len::<LEGACY_CCMP>(),
        (PeerQos::Qos, LinkProtection::Ccmp) => front_len::<QOS_CCMP>(),
    }
}

/// The TID of the user priority `UP`, checked at compile time.
const fn tid_of<const UP: u8>() -> Tid {
    const {
        match Tid::new(UP) {
            Some(tid) => tid,
            None => panic!("a user priority is a TID"),
        }
    }
}

/// Each user priority's TID, made once.
const TIDS: [Tid; 8] = [
    tid_of::<0>(),
    tid_of::<1>(),
    tid_of::<2>(),
    tid_of::<3>(),
    tid_of::<4>(),
    tid_of::<5>(),
    tid_of::<6>(),
    tid_of::<7>(),
];

/// One MSDU from the network: its Ethernet frame, its header the last
/// [`ETHERNET_HEADER_LEN`] octets of the memory's headroom and its payload
/// the body.
#[derive(Debug, Eq, PartialEq)]
pub struct OutboundMsdu<M> {
    /// The network's memory holding it.
    pub memory: M,
    /// The length of its Ethernet frame.
    pub length: usize,
    /// Its user priority.
    pub priority: WmmUserPriority,
}
/// What the access point's HT Capabilities ask of the station's A-MPDUs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtLimits {
    /// Its Minimum MPDU Start Spacing.
    pub spacing: MpduStartSpacing,
    /// Its Maximum A-MPDU Length, in octets.
    pub max_length: u32,
}

/// A TID's operational transmit Block Ack window: the oldest sequence
/// number it has not settled and its size, 1 to 64.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxWindow {
    /// The window's head.
    pub head: SequenceNumber,
    /// The window's size.
    pub size: u16,
}

impl TxWindow {
    /// Whether `sequence` lies beyond the window's end: after its head in
    /// modulo-4096 order, but not inside it. The originator sends no MPDU
    /// there until the window moves over the MPDUs before it.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 10.25.6 (HT-immediate block ack: the
    /// originator transmits within WinStartO to WinStartO + WinSizeO - 1)
    /// and 10.24.7 (the half-space horizon of sequence comparisons).
    fn ahead(&self, sequence: SequenceNumber) -> bool {
        let distance = self.head.forward_distance(sequence);
        distance >= self.size && distance < SequenceNumber::HALF_SPACE
    }
}

/// One link's data state, handed to the plane at connect and returned at
/// stop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataLink {
    /// The prefix of every data MPDU of the association.
    pub header: StaDataHeaderTemplate,
    /// The access categories the link's EDCA parameters close.
    pub admission: AcAdmission,
    /// How long a frame a BlockAck missed is retried; `None` for a link
    /// without transmit agreements, whose frames never aggregate.
    pub retry: Option<AggregateRetry>,
    /// The access point's A-MPDU limits; `None` for a non-HT access point.
    pub ht: Option<HtLimits>,
    /// Each TID's operational transmit Block Ack window.
    pub windows: [Option<TxWindow>; 8],
    /// The protection facts of the access point's BSS; the publications
    /// follow its HE TXOP Duration RTS Threshold
    /// ([`ProtectionPolicy::every_he_txop_protection`](oer_protocols_ieee80211_core_upper_mac::ProtectionPolicy::every_he_txop_protection)).
    pub protection: oer_protocols_ieee80211_core_upper_mac::protection::BssProtection,
}

/// Why the plane did not take a link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandoverRefused {
    /// The plane holds a link already.
    Connected,
}

/// The port's A-MPDU limits at the rate the aggregate rate selection
/// gives now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AggregateRate {
    /// The aggregate rate.
    pub rate: PhyRate,
    /// The most subframes the port aggregates.
    pub max_subframes: u16,
    /// The port's longest A-MPDU at the rate, in octets.
    pub max_length: u32,
}

/// Why the plane refused an MSDU; its memory goes back to the network.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntakeRefusal {
    /// The plane holds no link.
    NoLink,
    /// The protected link holds no key.
    NoKey,
    /// The link's sequence spaces are not those its template's QoS names.
    Numbering,
    /// Admission control closes the MSDU's access category and every lower
    /// one.
    NoAdmittedAccessCategory,
    /// The frame is shorter than an Ethernet header.
    ShortFrame,
    /// The MPDU is longer than the port sends.
    TooLong,
    /// The key's packet numbers are used up.
    PacketNumberExhausted,
}

/// What the plane did with an MSDU.
#[derive(Debug, Eq, PartialEq)]
pub enum Intake<M> {
    /// The standby arena took it as one MPDU of this TID.
    Taken(WmmUserPriority),
    /// The standby cannot take it now, for the reason given. The MSDU,
    /// unchanged, waits in its queue.
    Wait(OutboundMsdu<M>, Waits),
    /// The MSDU ends: its memory goes back to the network.
    Refused {
        /// The memory it came in.
        memory: M,
        /// Why.
        reason: IntakeRefusal,
    },
}

/// Why the standby cannot take an MSDU now, and so what frees it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Waits {
    /// No arena is free, or the standby is full: the next settlement frees
    /// one, for any TID.
    Arena,
    /// The standby holds another TID's frames, or this one's do not follow:
    /// the next arena takes it.
    Standby,
    /// Its sequence number lies beyond its TID's window: only that TID
    /// waits, until a settlement or a new agreement moves the window.
    Window,
}

/// How an arena goes on the air, as the plane planned it; the caller gives
/// it its rate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PpduKind {
    /// One MPDU that solicits an ACK.
    Single,
    /// An A-MPDU under the TID's agreement, at the recipient's spacing.
    Aggregate {
        /// The recipient's Minimum MPDU Start Spacing.
        spacing: MpduStartSpacing,
    },
}

/// The publication the plane plans: the caller's rate plan turns it into
/// a [`Ppdu`] ([`DataPlane::publication`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PpduPlan {
    /// The arena that goes.
    pub arena: ArenaSide,
    /// The TID of its frames.
    pub tid: WmmUserPriority,
    /// Its frames.
    pub frames: u8,
    /// Single MPDU or A-MPDU.
    pub kind: PpduKind,
}

impl PpduPlan {
    /// The rate record the plan's rate comes from.
    pub const fn frame_rates(&self) -> FrameRates {
        match self.kind {
            PpduKind::Single => FrameRates::Data,
            PpduKind::Aggregate { .. } => FrameRates::Aggregate,
        }
    }
}

/// What one arena's end teaches the rate selection, in the outcome form it
/// reads: a single MPDU's with its ACK evidence `A`, or an A-MPDU's with
/// its BlockAck evidence `B` and tally.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateLesson<A, B, D> {
    /// A single MPDU of the network's data ended.
    Single(SingleOutcome<A, D>),
    /// An A-MPDU of the network's data ended.
    Aggregate {
        /// Its subframes and those the BlockAck acknowledged.
        tally: BlockAckTally,
        /// How it ended.
        outcome: AggregateOutcome<B, D>,
    },
}

impl<A: Copy, B: Copy, D: Copy> RateLesson<A, B, D> {
    /// Teach `rates` at `now`, on its rules for a current link's terminal:
    /// only an outcome with a rate sample, or an A-MPDU that went on the
    /// air, teaches it. A single MPDU's response evidence reaches the
    /// selection as its ACK evidence through `ack`; one `ack` cannot read
    /// teaches nothing.
    pub fn teach<M: RxMetadata, E, S: RateSelection<M, E, B>>(
        &self,
        now: Instant,
        rates: &mut S,
        ack: impl Fn(A) -> Option<E>,
    ) {
        match self {
            Self::Single(outcome) => {
                if let Some(outcome) = single_with(*outcome, ack) {
                    rates.learn(now, FrameRates::Data, true, &outcome);
                }
            }
            Self::Aggregate { tally, outcome } => {
                rates.learn_aggregate(now, true, *tally, outcome);
            }
        }
    }
}

/// `outcome` with its response evidence read through `ack`; `None` when
/// `ack` cannot read it.
fn single_with<A, E, D>(
    outcome: SingleOutcome<A, D>,
    ack: impl Fn(A) -> Option<E>,
) -> Option<SingleOutcome<E, D>> {
    Some(match outcome {
        TxOutcome::Ended(SingleEnd::Delivered { air, response }) => {
            let response = match response {
                SingleResponse::Unsolicited => SingleResponse::Unsolicited,
                SingleResponse::Ack(response) => SingleResponse::Ack(ack(response)?),
                SingleResponse::BlockAck {
                    report,
                    ack: response,
                } => SingleResponse::BlockAck {
                    report,
                    ack: ack(response)?,
                },
            };
            TxOutcome::Ended(SingleEnd::Delivered { air, response })
        }
        TxOutcome::Ended(SingleEnd::AckTimeout(air)) => {
            TxOutcome::Ended(SingleEnd::AckTimeout(air))
        }
        TxOutcome::Ended(SingleEnd::CtsTimeout(air)) => {
            TxOutcome::Ended(SingleEnd::CtsTimeout(air))
        }
        TxOutcome::Ended(SingleEnd::ContentionLimit(air)) => {
            TxOutcome::Ended(SingleEnd::ContentionLimit(air))
        }
        TxOutcome::Ended(SingleEnd::AirDenied { denial, air }) => {
            TxOutcome::Ended(SingleEnd::AirDenied { denial, air })
        }
        TxOutcome::Expired { msdus } => TxOutcome::Expired { msdus },
        TxOutcome::Withdrawn(cause) => TxOutcome::Withdrawn(cause),
        TxOutcome::Indeterminate(cause) => TxOutcome::Indeterminate(cause),
    })
}

/// What one arena's end settled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Settled<A, B, D> {
    /// The BlockAckReq owed: a frame was dropped under the TID's agreement,
    /// whose window moved to this head.
    pub bar: Option<(WmmUserPriority, SequenceNumber)>,
    /// What the end teaches the rate selection; `None` for an arena of an
    /// ended link.
    pub lesson: Option<RateLesson<A, B, D>>,
    /// Frames acknowledged.
    pub delivered: u8,
    /// Frames dropped.
    pub dropped: u8,
    /// Frames the arena keeps for its next publication: those kept for a
    /// retry and those that did not go.
    pub retried: u8,
    /// The TID the arena held, and the cost of the attempt that ended: one
    /// charge for the whole PPDU ([`crate::cost`]).
    pub tid: WmmUserPriority,
    pub cost: TxEstimate,
    /// The other arena's publication, put on the air ahead of this
    /// settlement because the BlockAck acknowledged every frame that went.
    pub ahead: Option<PpduPlan>,
}

/// An arena's right, in its place in the book.
#[derive(Debug)]
enum Held<'id> {
    /// It holds nothing.
    Idle(IdleArena<'id>),
    /// It takes or keeps frames.
    Open(Arena<'id>),
    /// It is on the air.
    Air(OnAir<'id>),
}

/// What the plane knows of the frames one arena holds. Frames are named by
/// their original position, the order intake took them in: position `o`
/// carries the sequence number `first + o`. The arena holds the positions
/// of `pending` in their order; a settlement compacts the arena, not the
/// book.
struct ArenaBook<'id, K> {
    /// The arena's right. `None` while the data is stopped (the
    /// installation holds the arena), and inside a transition of the plane,
    /// which puts the next right back before it returns.
    right: Option<Held<'id>>,
    /// The link generation that filled it.
    epoch: u32,
    tid: WmmUserPriority,
    /// The sequence number of original position 0.
    first: SequenceNumber,
    /// Original positions taken.
    taken: u8,
    /// Original positions the arena still holds.
    pending: u64,
    /// The subframe octets of the taken frames.
    octets: u32,
    /// What the book knows of each original position's frame.
    frames: [BookFrame; BOOK_FRAMES],
    /// The arena holds frames to retry and takes no more.
    retained: bool,
    /// How the publication on the air goes.
    published: Option<Published<'id, K>>,
}

/// What a book knows of one frame.
#[derive(Clone, Copy)]
struct BookFrame {
    /// Its PSDU on the air as one MPDU: the MPDU, its CCMP MIC and its FCS.
    psdu: u16,
    /// When its MSDU was admitted.
    admitted: Instant,
    /// The publications it went in that may have reached a receiver: a
    /// frame's attempts, not the arena's.
    attempts: u8,
}

impl BookFrame {
    const EMPTY: Self = Self {
        psdu: 0,
        admitted: Instant::EPOCH,
        attempts: 0,
    };
}

/// The publication on the air, and what its end is charged by, read from
/// the book when it went.
struct Published<'id, K> {
    ppdu: Ppdu<'id, K>,
    /// The PSDU octets of the frames that went: a single's, which is one.
    psdu: u32,
    /// The subframe octets of the frames that went: an aggregate's.
    octets: u32,
}

impl<K> Clone for Published<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for Published<'_, K> {}

/// A frame's original position in its book, below [`BOOK_FRAMES`] by its
/// constructor: its bit in a book's `pending` and its entry exist.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Position(u8);

impl Position {
    /// `position`, when a book has it.
    const fn new(position: u8) -> Option<Self> {
        if (position as usize) < BOOK_FRAMES {
            Some(Self(position))
        } else {
            None
        }
    }

    /// Its bit in a book's frame mask; the shift is below 64.
    const fn bit(self) -> u64 {
        1_u64.wrapping_shl(self.0 as u32)
    }
}

/// The number of frames of a book's mask: at most 64, which a byte holds.
const fn ones(bits: u64) -> u8 {
    bits.count_ones() as u8
}

impl<'id, K> ArenaBook<'id, K> {
    const fn new() -> Self {
        Self {
            right: None,
            epoch: 0,
            tid: WmmUserPriority::UP0,
            first: SequenceNumber::from_low_bits(0),
            taken: 0,
            pending: 0,
            octets: 0,
            frames: [BookFrame::EMPTY; BOOK_FRAMES],
            retained: false,
            published: None,
        }
    }

    /// Frames the arena holds.
    const fn count(&self) -> u8 {
        ones(self.pending)
    }

    /// The frames the arena holds with their positions, in its order.
    fn held(&self) -> impl Iterator<Item = (Position, &BookFrame)> {
        let pending = self.pending;
        // The entries are BOOK_FRAMES: each index is a position.
        self.frames
            .iter()
            .zip(0_u8..)
            .map(|(frame, position)| (Position(position), frame))
            .filter(move |(position, _)| pending & position.bit() != 0)
    }

    /// The same, each frame's entry to change.
    fn held_mut(&mut self) -> impl Iterator<Item = (Position, &mut BookFrame)> {
        let pending = self.pending;
        // The entries are BOOK_FRAMES: each index is a position.
        self.frames
            .iter_mut()
            .zip(0_u8..)
            .map(|(frame, position)| (Position(position), frame))
            .filter(move |(position, _)| pending & position.bit() != 0)
    }

    /// Record `frame` at `position`.
    fn set(&mut self, position: Position, frame: BookFrame) {
        for (entry, at) in self.frames.iter_mut().zip(0_u8..) {
            if at == position.0 {
                *entry = frame;
            }
        }
    }

    /// The arena holds no frame: its book starts again.
    const fn clear(&mut self) {
        self.taken = 0;
        self.pending = 0;
        self.octets = 0;
        self.retained = false;
    }

    /// Whether its arena is on the air.
    const fn on_air(&self) -> bool {
        matches!(self.right, Some(Held::Air(_)))
    }

    /// The arena is the standby: it is not on the air and takes frames.
    const fn takes(&self) -> bool {
        matches!(self.right, Some(Held::Idle(_) | Held::Open(_))) && !self.retained
    }

    /// Whether it holds frames the next publication may put on the air.
    const fn publishable(&self) -> bool {
        matches!(self.right, Some(Held::Open(_))) && self.pending != 0
    }

    /// The oldest sequence number the arena holds of `tid` for the link
    /// generation `epoch`.
    fn oldest(&self, epoch: u32, tid: WmmUserPriority) -> Option<SequenceNumber> {
        if self.epoch != epoch || self.tid != tid || self.pending == 0 {
            return None;
        }
        let (oldest, _) = self.held().next()?;
        Some(self.first.wrapping_add(u16::from(oldest.0)))
    }
}

/// The octets one frame of `psdu` takes in an aggregate: padded to four
/// and its delimiter (IEEE Std 802.11-2020, 9.7.1).
const fn subframe(psdu: u16) -> u32 {
    const DELIMITER: u32 = 4;
    // A `u16` padded to four and its delimiter stay far within a `u32`.
    ((psdu as u32).saturating_add(3) & !3).saturating_add(DELIMITER)
}

/// The connected station's data plane over a port's two arenas.
pub struct DataPlane<'id, K> {
    link: Option<DataLink>,
    /// The generation of the current link, which each book records.
    epoch: u32,
    /// The port's A-MPDU limits at the aggregate rate.
    rate: Option<AggregateRate>,
    /// Each TID's A-MPDU bound: with its window, the access point's HT
    /// limits and the aggregate rate.
    limits: [Option<PpduLimit>; 8],
    /// Each TID's sequence number after the last MPDU the link's data drew.
    next: [Option<SequenceNumber>; 8],
    first: ArenaBook<'id, K>,
    second: ArenaBook<'id, K>,
}

impl<K: Ieee80211Capabilities> Default for DataPlane<'_, K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'id, K: Ieee80211Capabilities> DataPlane<'id, K> {
    /// The plane of stopped data: it holds no arena until
    /// [`Self::give`].
    pub const fn new() -> Self {
        Self {
            link: None,
            epoch: 0,
            rate: None,
            limits: [None; 8],
            next: [None; 8],
            first: ArenaBook::new(),
            second: ArenaBook::new(),
        }
    }

    /// The data flows: the installation's two idle arenas, each to its
    /// book.
    pub fn give(&mut self, arenas: [IdleArena<'id>; 2]) {
        let [first, second] = arenas;
        self.first.right = Some(Held::Idle(first));
        self.first.clear();
        self.second.right = Some(Held::Idle(second));
        self.second.clear();
    }

    /// The plane's whole state but its frames' admission instants, for the
    /// engine's search: no lifetime expires within its bounds.
    #[cfg(test)]
    pub(crate) fn key(&self) -> std::string::String {
        let book = |book: &ArenaBook<'id, K>| {
            let right = match &book.right {
                None => std::string::String::from("away"),
                Some(Held::Idle(_)) => std::string::String::from("idle"),
                Some(Held::Open(arena)) => std::format!("open{}", arena.frames()),
                Some(Held::Air(air)) => std::format!("air{}", air.sent().bits()),
            };
            let frames: std::vec::Vec<(u8, u16, u8)> = book
                .held()
                .map(|(position, frame)| (position.0, frame.psdu, frame.attempts))
                .collect();
            std::format!(
                "{right}|{}|{:?}|{:?}|{}|{}|{}|{:?}|{}|{:?}",
                book.epoch,
                book.tid,
                book.first,
                book.taken,
                book.pending,
                book.octets,
                frames,
                book.retained,
                book.published
                    .map(|published| (published.psdu, published.octets)),
            )
        };
        std::format!(
            "{:?}|{}|{:?}|{:?}|{:?}|{}|{}",
            self.link,
            self.epoch,
            self.rate,
            self.limits,
            self.next,
            book(&self.first),
            book(&self.second)
        )
    }

    /// The two arenas back, when both are idle: the data stops. `None`
    /// while one holds frames or is on the air; the plane keeps them.
    pub fn take(&mut self) -> Option<[IdleArena<'id>; 2]> {
        match (self.first.right.take(), self.second.right.take()) {
            (Some(Held::Idle(first)), Some(Held::Idle(second))) => Some([first, second]),
            (first, second) => {
                self.first.right = first;
                self.second.right = second;
                None
            }
        }
    }

    /// The book of the arena of `side`: each book is its own arena's.
    const fn book(&self, side: ArenaSide) -> &ArenaBook<'id, K> {
        match side {
            ArenaSide::First => &self.first,
            ArenaSide::Second => &self.second,
        }
    }

    const fn book_mut(&mut self, side: ArenaSide) -> &mut ArenaBook<'id, K> {
        match side {
            ArenaSide::First => &mut self.first,
            ArenaSide::Second => &mut self.second,
        }
    }

    /// Whether the plane holds a link.
    pub const fn is_connected(&self) -> bool {
        self.link.is_some()
    }

    /// The TID an MSDU of user priority `priority` goes in on the plane's
    /// link: its admitted user priority on a QoS link, UP0 otherwise;
    /// `None` without a link or an admitted access category.
    pub fn tid(&self, priority: WmmUserPriority) -> Option<WmmUserPriority> {
        let link = self.link.as_ref()?;
        if link.header.peer_qos() == PeerQos::Qos {
            link.admission.admitted_user_priority(priority)
        } else {
            Some(WmmUserPriority::UP0)
        }
    }

    /// The operational window of `tid`.
    pub fn window(&self, tid: WmmUserPriority) -> Option<TxWindow> {
        *tid.select(&self.link.as_ref()?.windows)
    }

    /// Take the link's data state. The plane starts it without aggregate
    /// rate ([`Self::aggregate_rate`]).
    ///
    /// # Errors
    ///
    /// The link, unchanged, when the plane holds one.
    #[expect(
        clippy::result_large_err,
        reason = "a refused link comes back by value; the plane has no allocator"
    )]
    pub fn handover(&mut self, link: DataLink) -> Result<(), (DataLink, HandoverRefused)> {
        if self.link.is_some() {
            return Err((link, HandoverRefused::Connected));
        }
        self.epoch = self.epoch.wrapping_add(1);
        self.rate = None;
        self.limits = [None; 8];
        self.next = [None; 8];
        self.link = Some(link);
        Ok(())
    }

    /// End the link: every frame an arena holds unpublished goes back to the
    /// network through `returned`, counted, and the link's data state, its
    /// agreements' windows with it, comes back to its holder. The arena on
    /// the air settles at its end, its frames released.
    pub fn stop<P: DataPort<'id, Caps = K>>(
        &mut self,
        port: &mut P,
        mut returned: impl FnMut(P::Memory),
        events: &mut impl EventSink<DataEvent>,
    ) -> Option<DataLink> {
        for side in [ArenaSide::First, ArenaSide::Second] {
            self.withdraw(port, side, &mut returned, events);
        }
        self.rate = None;
        self.limits = [None; 8];
        self.next = [None; 8];
        self.link.take()
    }

    /// Return every frame the arenas hold unpublished, keeping the link:
    /// the frames written under the link's key before a new one replaces it
    /// carry that key's packet numbers.
    pub fn withdraw_unpublished<P: DataPort<'id, Caps = K>>(
        &mut self,
        port: &mut P,
        mut returned: impl FnMut(P::Memory),
        events: &mut impl EventSink<DataEvent>,
    ) {
        for side in [ArenaSide::First, ArenaSide::Second] {
            self.withdraw(port, side, &mut returned, events);
        }
    }

    /// Whether an arena is on the air: its end is owed.
    pub const fn on_air(&self) -> bool {
        self.first.on_air() || self.second.on_air()
    }

    /// While an arena is on the air, the frames the standby still takes
    /// before its PPDU bound, of the arena's `capacity`: the bound of the
    /// TID it holds, or the whole arena while it holds none. `None` while
    /// no arena is on the air: the next frame goes out at once.
    pub fn standby_room(&self, capacity: u8) -> Option<u8> {
        if !self.on_air() {
            return None;
        }
        let Some(side) = self.standby() else {
            return Some(0);
        };
        let book = self.book(side);
        let count = book.count();
        if count == 0 {
            return Some(capacity);
        }
        let bound = (*book.tid.select(&self.limits))
            .map_or(0, |limit| limit.subframe_count().min(capacity));
        Some(bound.saturating_sub(count))
    }

    /// The frames the arenas hold: published, retained or taken.
    pub const fn frames(&self) -> u8 {
        self.first.count().saturating_add(self.second.count())
    }

    /// Whether an arena holds a frame: published, retained or taken.
    pub const fn holds_frames(&self) -> bool {
        self.first.pending != 0 || self.second.pending != 0
    }

    /// Return every frame the arena of `side` holds while it is not on the
    /// air; its right goes back to idle.
    fn withdraw<P: DataPort<'id, Caps = K>>(
        &mut self,
        port: &mut P,
        side: ArenaSide,
        returned: &mut impl FnMut(P::Memory),
        events: &mut impl EventSink<DataEvent>,
    ) {
        let book = self.book_mut(side);
        let right = match book.right.take() {
            Some(Held::Open(arena)) => {
                let mut leave = |memory| {
                    report(events, || DataEvent::DataLost(DataLoss::LeftBehind));
                    returned(memory);
                };
                Held::Idle(port.withdraw(arena, &mut leave))
            }
            Some(held @ (Held::Idle(_) | Held::Air(_))) => {
                book.right = Some(held);
                return;
            }
            None => return,
        };
        book.right = Some(right);
        book.clear();
    }

    /// The port's A-MPDU limits at the aggregate rate changed: every TID's
    /// bound is computed again, once.
    pub fn aggregate_rate(&mut self, rate: Option<AggregateRate>) {
        if self.rate == rate {
            return;
        }
        self.rate = rate;
        for tid in WmmUserPriority::ALL {
            self.limit(tid);
        }
    }

    /// Compute `tid`'s A-MPDU bound again.
    fn limit(&mut self, tid: WmmUserPriority) {
        let limit = self.link.as_ref().and_then(|link| {
            let window = (*tid.select(&link.windows))?;
            let ht = link.ht?;
            let rate = self.rate?;
            let subframes = window.size.min(rate.max_subframes);
            Some(PpduLimit::new(
                usize::from(subframes),
                rate.max_length.min(ht.max_length),
                Some(rate.rate),
            ))
        });
        *tid.select_mut(&mut self.limits) = limit;
    }

    /// Whether `tid` aggregates: its agreement is operational on an HT link.
    fn aggregates(&self, tid: WmmUserPriority) -> bool {
        self.link
            .as_ref()
            .is_some_and(|link| link.ht.is_some() && tid.select(&link.windows).is_some())
    }

    /// `tid`'s transmit agreement became operational with `window`, or
    /// ended (`None`). An arena of more than one frame of a TID without
    /// agreement goes as single MPDUs, one frame at a time.
    pub fn agreement(&mut self, tid: WmmUserPriority, window: Option<TxWindow>) {
        let Some(link) = self.link.as_mut() else {
            return;
        };
        *tid.select_mut(&mut link.windows) = window;
        self.limit(tid);
    }

    /// The arena the next frame goes to: the one filled, else an empty one
    /// the plane holds.
    fn standby(&self) -> Option<ArenaSide> {
        let sides = [ArenaSide::First, ArenaSide::Second];
        sides
            .into_iter()
            .find(|side| self.book(*side).takes() && self.book(*side).pending != 0)
            .or_else(|| sides.into_iter().find(|side| self.book(*side).takes()))
    }

    /// Whether an arena takes frames now.
    pub fn takes(&self) -> bool {
        self.standby().is_some()
    }

    /// The TID whose frames the standby holds: only its next frames join
    /// them. `None` while the standby holds none, or no arena takes frames.
    pub fn standby_tid(&self) -> Option<WmmUserPriority> {
        let book = self.book(self.standby()?);
        (book.count() != 0).then_some(book.tid)
    }
}

impl<'id, K: Ieee80211Capabilities> DataPlane<'id, K> {
    /// Take `msdu`, admitted at `now`, into the standby arena of `port` as
    /// one MPDU of the port's `link`: its body stays in place and its front
    /// is written into the headroom from the link's template, with the next
    /// sequence number of its TID in `numbers` and, on a protected link,
    /// the CCMP header of the key's next packet number. No sequence number
    /// is drawn for an MSDU that waits or is refused.
    pub fn intake<P: DataPort<'id, Caps = K>, T: TxKey>(
        &mut self,
        port: &mut P,
        link: &P::Link,
        msdu: OutboundMsdu<P::Memory>,
        numbers: &mut TxNumbers<T>,
        now: Instant,
    ) -> Intake<P::Memory> {
        let refused = |msdu: OutboundMsdu<P::Memory>, reason| Intake::Refused {
            memory: msdu.memory,
            reason,
        };
        let Some(data) = self.link.as_ref() else {
            return refused(msdu, IntakeRefusal::NoLink);
        };
        let header = data.header;
        // The final priority names the frame's TID, its sequence space and
        // its access category; a non-QoS link has one queue, best effort.
        let tid = if header.peer_qos() == PeerQos::Qos {
            match data.admission.admitted_user_priority(msdu.priority) {
                Some(tid) => tid,
                None => return refused(msdu, IntakeRefusal::NoAdmittedAccessCategory),
            }
        } else {
            WmmUserPriority::UP0
        };
        let protection = match header.protection() {
            LinkProtection::Open => DataProtection::Open,
            LinkProtection::Ccmp => DataProtection::Ccmp(()),
        };
        if (header.peer_qos() == PeerQos::Qos) != numbers.qos() {
            return refused(msdu, IntakeRefusal::Numbering);
        }
        let key = numbers.key().map(TxKey::packet_numbers);
        match (protection, key) {
            (DataProtection::Ccmp(()), None) => return refused(msdu, IntakeRefusal::NoKey),
            (DataProtection::Ccmp(()), Some(key)) if !key.available() => {
                return refused(msdu, IntakeRefusal::PacketNumberExhausted);
            }
            (DataProtection::Ccmp(()), Some(_)) | (DataProtection::Open, _) => {}
        }
        let Some(payload) = msdu.length.checked_sub(ETHERNET_HEADER_LEN) else {
            return refused(msdu, IntakeRefusal::ShortFrame);
        };
        let mic = match protection {
            DataProtection::Open => 0,
            DataProtection::Ccmp(()) => CCMP_MIC_LEN,
        };
        let mpdu = header.prefix_len().checked_add(payload);
        let (Some(length), Some(octets), Some(psdu)) = (
            mpdu.and_then(MpduLength::<K>::new),
            mpdu.and_then(|mpdu| aggregate::subframe_octets(mpdu, protection)),
            mpdu.and_then(|mpdu| mpdu.checked_add(mic)?.checked_add(FCS_LEN))
                .and_then(|psdu| u16::try_from(psdu).ok()),
        ) else {
            return refused(msdu, IntakeRefusal::TooLong);
        };
        let sequence = numbers.next(tid);
        let window = *tid.select(&data.windows);
        let Some(side) = self.standby() else {
            return Intake::Wait(msdu, Waits::Arena);
        };
        if window.is_some_and(|window| window.ahead(sequence)) {
            return Intake::Wait(msdu, Waits::Window);
        }
        let book = self.book(side);
        let count = book.count();
        if count != 0 {
            let limit = *tid.select(&self.limits);
            let follows =
                book.tid == tid && u16::from(book.taken) == book.first.forward_distance(sequence);
            let fits = limit.is_some_and(|limit| {
                usize::from(count) < limit.subframes()
                    && book
                        .octets
                        .checked_add(octets)
                        .is_some_and(|total| total <= limit.octets())
            });
            if !follows {
                return Intake::Wait(msdu, Waits::Standby);
            }
            if !fits {
                return Intake::Wait(msdu, Waits::Arena);
            }
        }
        let epoch = self.epoch;
        let Self {
            first,
            second,
            next,
            ..
        } = self;
        let book = match side {
            ArenaSide::First => first,
            ArenaSide::Second => second,
        };
        // An empty arena opens for the frame's TID: one that is open and
        // holds nothing goes idle first, since it was opened for another.
        if count == 0 {
            let idle = match book.right.take() {
                Some(Held::Open(arena)) => Some(port.withdraw(arena, &mut |_memory| {})),
                Some(Held::Idle(idle)) => Some(idle),
                Some(air @ Held::Air(_)) => {
                    book.right = Some(air);
                    None
                }
                None => None,
            };
            if let Some(idle) = idle {
                book.right = Some(Held::Open(port.open(idle, link, *tid.select(&TIDS))));
            }
        }
        // A book names 64 positions: a book that took them all takes no
        // more until the arena empties, as a full arena.
        let Some(position) = Position::new(if count == 0 { 0 } else { book.taken }) else {
            return Intake::Wait(msdu, Waits::Arena);
        };
        let Some(Held::Open(arena)) = book.right.as_mut() else {
            return Intake::Wait(msdu, Waits::Arena);
        };
        let Ok(place) = port.place(arena, length) else {
            // Its slots are full: the next settlement frees one.
            return Intake::Wait(msdu, Waits::Arena);
        };
        let (counter, key) = numbers.data_and_key(tid);
        let ccmp = match (protection, key.map(TxKey::packet_numbers)) {
            (DataProtection::Open, _) => None,
            (DataProtection::Ccmp(()), Some(key)) => match key.allocate() {
                Ok(packet_number) => {
                    Some(CcmpHeader::new(packet_number, CcmpKeyId::PAIRWISE).encode())
                }
                // The place goes unwritten: the arena keeps its slot.
                Err(_) => return refused(msdu, IntakeRefusal::PacketNumberExhausted),
            },
            (DataProtection::Ccmp(()), None) => return refused(msdu, IntakeRefusal::NoKey),
        };
        let sequence_number = counter.take();
        place.write(msdu.memory, |headroom| {
            write_front(headroom, &header, tid, sequence_number, ccmp)
        });
        if count == 0 {
            book.epoch = epoch;
            book.tid = tid;
            book.first = sequence;
            book.clear();
        }
        book.set(
            position,
            BookFrame {
                psdu,
                admitted: now,
                attempts: 0,
            },
        );
        book.pending |= position.bit();
        book.taken = position.0.saturating_add(1);
        book.octets = book.octets.saturating_add(octets);
        *tid.select_mut(next) = Some(sequence.wrapping_add(1));
        Intake::Taken(tid)
    }

    /// Put the next arena on the air at `now`, as `how` gives the plane's
    /// plan its rates: the arena whose frames wait from an earlier
    /// publication first, then the standby; more frames than one as an
    /// A-MPDU under the TID's agreement, else a single MPDU of its first
    /// frame. The plan that went; `None` while an arena is on the air, none
    /// holds a frame, or `how` gives nothing of the plan's kind.
    pub fn publication<P: DataPort<'id, Caps = K>>(
        &mut self,
        port: &mut P,
        gate: &GateOpen<'id>,
        now: Instant,
        how: impl FnOnce(PpduPlan) -> Option<Ppdu<'id, K>>,
    ) -> Option<PpduPlan> {
        let side = self.publishable()?;
        self.publication_of(port, gate, now, side, how)
    }

    /// The publication of the arena of `side`, which holds a frame and is
    /// open.
    fn publication_of<P: DataPort<'id, Caps = K>>(
        &mut self,
        port: &mut P,
        gate: &GateOpen<'id>,
        now: Instant,
        side: ArenaSide,
        how: impl FnOnce(PpduPlan) -> Option<Ppdu<'id, K>>,
    ) -> Option<PpduPlan> {
        let link = self.link.as_ref()?;
        let book = self.book(side);
        let frames = book.count();
        // More frames than one of a TID without agreement go as single
        // MPDUs, its first frame each time.
        let kind = match link.ht {
            Some(ht) if frames > 1 && self.aggregates(book.tid) => PpduKind::Aggregate {
                spacing: ht.spacing,
            },
            _ => PpduKind::Single,
        };
        let plan = PpduPlan {
            arena: side,
            tid: book.tid,
            frames,
            kind,
        };
        // Every individually addressed HE PPDU of a BSS that advertises an
        // HE TXOP Duration RTS Threshold goes behind RTS/CTS, its retries
        // and an aggregate alike: the plane estimates no TXOP duration.
        let mut policy = oer_protocols_ieee80211_core_upper_mac::ProtectionPolicy::new(None);
        policy.install_bss(link.protection);
        let protect = |rate: &oer_protocols_ieee80211_contracts_port::PortRate<'id, K>| {
            policy.every_he_txop_protection(rate.get())
        };
        let ppdu = match (how(plan)?, kind) {
            (Ppdu::Single(single), PpduKind::Single) => {
                Ppdu::Single(single.with_protection(protect))
            }
            (Ppdu::Aggregate { rate, alone, .. }, PpduKind::Aggregate { spacing }) => {
                Ppdu::Aggregate {
                    rate,
                    spacing,
                    protection: policy.every_he_txop_protection(rate.rate().get()),
                    alone: alone.with_protection(protect),
                }
            }
            (Ppdu::Single(_), PpduKind::Aggregate { .. })
            | (Ppdu::Aggregate { .. }, PpduKind::Single) => return None,
        };
        let book = self.book_mut(side);
        let Some(Held::Open(arena)) = book.right.take() else {
            return None;
        };
        let on_air = port.publish(arena, ppdu, gate, now);
        // What went: the arena's first frames, as many as the port sent.
        let sent = on_air.sent().count();
        let (psdu, octets) = book.held().take(usize::from(sent)).fold(
            (0_u32, 0_u32),
            |(psdu, octets), (_, frame)| {
                (
                    psdu.saturating_add(u32::from(frame.psdu)),
                    octets.saturating_add(subframe(frame.psdu)),
                )
            },
        );
        book.right = Some(Held::Air(on_air));
        book.published = Some(Published { ppdu, psdu, octets });
        Some(plan)
    }

    /// The arena the next publication puts on the air: none while one is on
    /// the air, else the one whose frames wait from an earlier publication,
    /// else the standby when it holds a frame.
    pub fn publishable(&self) -> Option<ArenaSide> {
        if self.on_air() {
            return None;
        }
        let sides = [ArenaSide::First, ArenaSide::Second];
        sides
            .into_iter()
            .find(|side| self.book(*side).publishable() && self.book(*side).retained)
            .or_else(|| {
                sides
                    .into_iter()
                    .find(|side| self.book(*side).publishable())
            })
    }
}

/// The other arena's side.
const fn other(side: ArenaSide) -> ArenaSide {
    match side {
        ArenaSide::First => ArenaSide::Second,
        ArenaSide::Second => ArenaSide::First,
    }
}

/// The set of the one position `position`.
const fn only(position: u8) -> FrameSet {
    FrameSet::from_bits(match 1_u64.checked_shl(position as u32) {
        Some(bit) => bit,
        None => 0,
    })
}

/// Whether an aggregate's end may have reached a receiver.
const fn reached<B, D>(outcome: &AggregateOutcome<B, D>) -> bool {
    matches!(
        outcome,
        TxOutcome::Ended(
            AggregateEnd::Delivered { .. }
                | AggregateEnd::AckTimeout(_)
                | AggregateEnd::CtsTimeout(Some(_))
                | AggregateEnd::ContentionLimit(Some(_))
                | AggregateEnd::AirDenied { air: Some(_), .. }
        ) | TxOutcome::Indeterminate(_)
    )
}

/// The rate an aggregate's last transmission went at.
const fn aggregate_rate<B, D>(outcome: &AggregateOutcome<B, D>) -> Option<PhyRate> {
    match outcome {
        TxOutcome::Ended(
            AggregateEnd::Delivered { air, .. }
            | AggregateEnd::AckTimeout(air)
            | AggregateEnd::CtsTimeout(Some(air))
            | AggregateEnd::ContentionLimit(Some(air))
            | AggregateEnd::AirDenied { air: Some(air), .. },
        ) => Some(air.rate),
        TxOutcome::Ended(
            AggregateEnd::CtsTimeout(None)
            | AggregateEnd::ContentionLimit(None)
            | AggregateEnd::AirDenied { air: None, .. },
        )
        | TxOutcome::Expired { .. }
        | TxOutcome::Withdrawn(_)
        | TxOutcome::Indeterminate(_) => None,
    }
}

/// What a single MPDU's miss lost it to.
pub(crate) const fn single_loss<A, D>(outcome: &SingleOutcome<A, D>) -> DataLoss {
    match outcome {
        TxOutcome::Indeterminate(_) => DataLoss::Indeterminate,
        TxOutcome::Ended(SingleEnd::ContentionLimit(_)) => DataLoss::ContentionLimited,
        TxOutcome::Expired { .. } => DataLoss::Expired,
        TxOutcome::Withdrawn(_) => DataLoss::LeftBehind,
        TxOutcome::Ended(
            SingleEnd::Delivered { .. }
            | SingleEnd::AckTimeout(_)
            | SingleEnd::CtsTimeout(_)
            | SingleEnd::AirDenied { .. },
        ) => DataLoss::RetryLimit,
    }
}

impl<'id, K: Ieee80211Capabilities> DataPlane<'id, K> {
    /// Read the end of the arena on the air at `now` and settle it, in
    /// section 4's order: the end, then, when its BlockAck acknowledged
    /// every frame the arena held and `gate` is open, the other arena
    /// published at once as `how` gives its rates, then the settlement. Acknowledged frames are
    /// delivered; an A-MPDU's missing ones are kept in their places while
    /// their MSDU's lifetime and the policy's attempts last, dropped after;
    /// a single MPDU's miss is dropped, its plan having been its retries;
    /// frames that did not go stay with no attempt spent. Every memory that
    /// leaves the arena goes back to the network through `returned`, in
    /// position order. An arena of an ended link releases every frame.
    /// `None` while no arena is on the air or its end has not come.
    #[expect(
        clippy::too_many_arguments,
        reason = "the port, its facts, its gate and instant, the next plan, the network's return and the events"
    )]
    pub fn completed<P>(
        &mut self,
        port: &mut P,
        facts: TxFacts,
        gate: Option<&GateOpen<'id>>,
        now: Instant,
        how: impl FnOnce(PpduPlan) -> Option<Ppdu<'id, K>>,
        mut returned: impl FnMut(P::Memory),
        events: &mut impl EventSink<DataEvent>,
    ) -> Option<Settled<P::Ack, P::BlockAck, P::Denial>>
    where
        P: DataPort<'id, Caps = K, Ack: Copy, BlockAck: Copy, Denial: Copy>,
    {
        let side = [ArenaSide::First, ArenaSide::Second]
            .into_iter()
            .find(|side| self.book(*side).on_air())?;
        let (outcome, sent, acknowledged) = {
            let Some(Held::Air(on_air)) = &self.book(side).right else {
                return None;
            };
            let ended = port.aggregate_ended(facts, on_air, now)?;
            (*ended.outcome, ended.sent, ended.acknowledged)
        };
        let book = self.book(side);
        let published = book.published?;
        let tid = book.tid;
        let count = book.count();
        let current = self.link.is_some() && book.epoch == self.epoch;
        let cost = match &outcome {
            ArenaOutcome::Single(single) => {
                let (Ppdu::Single(plan) | Ppdu::Aggregate { alone: plan, .. }) = published.ppdu;
                single_cost(&plan, published.psdu, single)
            }
            ArenaOutcome::Aggregate(aggregate) => aggregate_cost(published.octets, aggregate),
        };
        let whole = current
            && matches!(
                outcome,
                ArenaOutcome::Aggregate(TxOutcome::Ended(AggregateEnd::Delivered { .. }))
            )
            && acknowledged == sent
            && sent.len() == u32::from(count);
        let ahead = match gate {
            Some(gate) if whole && self.book(other(side)).publishable() => {
                self.publication_of(port, gate, now, other(side), how)
            }
            _ => None,
        };
        let retry = self.link.as_ref().and_then(|link| link.retry);
        let book = self.book_mut(side);
        let Some(Held::Air(on_air)) = book.right.take() else {
            return None;
        };
        book.published = None;
        if !current {
            // An ended link's frames neither retry nor move its windows.
            let right = match port.settle(on_air, FrameSet::EMPTY, &mut returned) {
                Kept::Kept(arena) => port.withdraw(arena, &mut returned),
                Kept::Idle(idle) => idle,
            };
            book.right = Some(Held::Idle(right));
            book.clear();
            return Some(Settled {
                bar: None,
                lesson: None,
                delivered: 0,
                dropped: 0,
                retried: 0,
                tid,
                cost,
                ahead,
            });
        }
        let sent_count = sent.count();
        let mut decision = Decision::default();
        match &outcome {
            ArenaOutcome::Single(single) => {
                if acknowledged.contains(0) {
                    decision.delivered = 1;
                } else {
                    report(events, || DataEvent::DataLost(single_loss(single)));
                    decision.dropped = 1;
                }
            }
            ArenaOutcome::Aggregate(aggregate) => {
                let reached = reached(aggregate);
                let mut first = true;
                for (position, (original, frame)) in (0..sent_count).zip(book.held_mut()) {
                    frame.attempts = frame.attempts.saturating_add(u8::from(reached));
                    let attempts = frame.attempts;
                    first &= attempts == 1;
                    if acknowledged.contains(position) {
                        decision.delivered = decision.delivered.saturating_add(1);
                        continue;
                    }
                    decision.missed = decision.missed.union(only(position));
                    let admitted = frame.admitted;
                    let loss = if matches!(aggregate, TxOutcome::Indeterminate(_)) {
                        Some(DataLoss::Indeterminate)
                    } else if retry.is_some_and(|retry| {
                        now.checked_duration_since(admitted)
                            .is_some_and(|age| age >= retry.lifetime)
                    }) {
                        Some(DataLoss::Expired)
                    } else if retry.is_none_or(|retry| attempts >= retry.attempts) {
                        Some(DataLoss::RetryLimit)
                    } else {
                        None
                    };
                    match loss {
                        Some(loss) => {
                            report(events, || DataEvent::DataLost(loss));
                            decision.dropped = decision.dropped.saturating_add(1);
                        }
                        None => {
                            decision.keep = decision.keep.union(only(position));
                            decision.pending |= original.bit();
                        }
                    }
                }
                decision.first =
                    first && matches!(aggregate, TxOutcome::Ended(AggregateEnd::Delivered { .. }));
            }
        }
        // The frames that did not go stay, in their order.
        for (original, _) in book.held().skip(usize::from(sent_count)) {
            decision.pending |= original.bit();
        }
        let retried = ones(decision.pending);
        let right = match port.settle(on_air, decision.keep, &mut returned) {
            Kept::Kept(arena) => Held::Open(arena),
            Kept::Idle(idle) => Held::Idle(idle),
        };
        book.right = Some(right);
        if decision.pending == 0 {
            book.clear();
        } else {
            book.pending = decision.pending;
            book.retained = true;
        }
        let lesson = match outcome {
            ArenaOutcome::Single(single) => Some(RateLesson::Single(single)),
            ArenaOutcome::Aggregate(aggregate) => {
                let tally = core::num::NonZeroU16::new(u16::from(sent_count))
                    .and_then(|went| BlockAckTally::new(went, u16::from(decision.delivered)).ok());
                tally.map(|tally| RateLesson::Aggregate {
                    tally,
                    outcome: aggregate,
                })
            }
        };
        if let ArenaOutcome::Aggregate(aggregate) = &outcome {
            report(events, || {
                DataEvent::AggregateSettled(AggregateSettled {
                    members: sent_count,
                    acknowledged: decision.delivered,
                    retried: decision.keep.count(),
                    dropped: decision.dropped,
                    refused: false,
                    holes: decision.first.then_some(decision.missed.bits()),
                    rate: aggregate_rate(aggregate),
                })
            });
        }
        let head = self.advance(tid);
        Some(Settled {
            bar: head
                .filter(|_| decision.dropped != 0)
                .map(|head| (tid, head)),
            lesson,
            delivered: decision.delivered,
            dropped: decision.dropped,
            retried,
            tid,
            cost,
            ahead,
        })
    }

    /// The oldest sequence number the arenas hold of `tid` on the current
    /// link.
    pub fn oldest_held(&self, tid: WmmUserPriority) -> Option<SequenceNumber> {
        let epoch = self.epoch;
        [
            self.first.oldest(epoch, tid),
            self.second.oldest(epoch, tid),
        ]
        .into_iter()
        .flatten()
        .reduce(|oldest, held| if held.precedes(oldest) { held } else { oldest })
    }

    /// Move `tid`'s window to the oldest sequence number the arenas still
    /// hold of it, else to the one after the last its data drew; never
    /// back. Its head, when `tid` has a window and its data drew a number.
    fn advance(&mut self, tid: WmmUserPriority) -> Option<SequenceNumber> {
        let epoch = self.epoch;
        let oldest = [
            self.first.oldest(epoch, tid),
            self.second.oldest(epoch, tid),
        ];
        let next = (*tid.select(&self.next))?;
        let link = self.link.as_mut()?;
        let head =
            oldest.into_iter().flatten().fold(
                next,
                |head, held| if held.precedes(head) { held } else { head },
            );
        let window = tid.select_mut(&mut link.windows).as_mut()?;
        if window.head.precedes(head) {
            window.head = head;
        }
        Some(window.head)
    }
}

/// What one end decides of an arena's frames.
#[derive(Default)]
struct Decision {
    delivered: u8,
    dropped: u8,
    /// Arena positions of the frames that went and are kept for a retry.
    keep: FrameSet,
    /// The original positions the arena holds after the settlement.
    pending: u64,
    /// The positions this attempt left unacknowledged.
    missed: FrameSet,
    /// Every frame that went was at its first transmission, and the
    /// aggregate was answered.
    first: bool,
}

/// Write the front of the MPDU of `header` into `headroom`, flush against
/// its body, from the Ethernet header the network left at the headroom's
/// end: the MAC header with `sequence_number` and the TID `priority`, the
/// CCMP header `ccmp` on a protected link and LLC/SNAP. Its length.
fn write_front(
    headroom: &mut [u8; HEADROOM],
    header: &StaDataHeaderTemplate,
    priority: WmmUserPriority,
    sequence_number: SequenceNumber,
    ccmp: Option<[u8; CCMP_HEADER_LEN]>,
) -> HeaderLen {
    // The Ethernet header: destination, source, EtherType.
    let [.., d0, d1, d2, d3, d4, d5, _, _, _, _, _, _, e0, e1] = *headroom;
    let fields = StaDataHeaderFields {
        destination: [d0, d1, d2, d3, d4, d5],
        sequence_number,
        priority,
        ether_type: u16::from_be_bytes([e0, e1]),
    };
    header.write_before(fields, headroom);
    if let Some(ccmp) = ccmp {
        // The CCMP header follows the MAC header, before LLC/SNAP.
        let [.., c0, c1, c2, c3, c4, c5, c6, c7, _, _, _, _, _, _, _, _] = headroom;
        let [p0, p1, p2, p3, p4, p5, p6, p7] = ccmp;
        (*c0, *c1, *c2, *c3, *c4, *c5, *c6, *c7) = (p0, p1, p2, p3, p4, p5, p6, p7);
    }
    front_of(header)
}

#[cfg(test)]
mod tests;
