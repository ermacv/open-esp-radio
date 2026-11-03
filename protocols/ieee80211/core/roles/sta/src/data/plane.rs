//! The connected station's data plane over the port's two transmit arenas
//! ([`TxArena`]).
//!
//! The plane holds what one link's data needs ([`DataLink`], handed over at
//! connect and returned at stop): the header template, the access
//! categories the link admits, each TID's operational transmit Block Ack
//! window, the retry policy and the access point's HT limits. It also holds
//! the right to each arena and a book beside it: what the plane knows of the
//! frames the arena holds (their TID, first sequence number, subframe
//! octets, each MSDU's admission and the attempts on the air).
//!
//! The sequence spaces and the pairwise key's packet numbers are the link's
//! ([`TxNumbers`]), which its holder numbers the station's own frames from
//! too (EAPOL, Block Ack negotiation): each intake draws from them.
//!
//! An MSDU becomes an MPDU once, written into the network's memory as the
//! arena takes it ([`DataPlane::intake`]): its sequence number and packet
//! number are drawn there and its header and CCMP header are written from
//! the template in front of the payload, which a prefix shorter than
//! [`MSDU_PAYLOAD_OFFSET`] moves down to it, so the MPDU starts at the
//! frame region's first octet. The arena that is filled (the
//! standby) belongs to one TID at a time and holds consecutive sequence
//! numbers inside that TID's window, up to the TID's [`PpduLimit`] (one
//! addition and one comparison per frame) and the arena's slots; a TID
//! without an operational agreement takes one frame. An MSDU the standby
//! cannot take now waits in the network's queue ([`Intake::Wait`]), its
//! memory untouched.
//!
//! [`DataPlane::publication`] puts one arena on the air at a time, at the
//! rate one plan gives it: an arena holding frames that wait from an
//! earlier publication first, then the standby. More frames than one go as
//! an A-MPDU under the TID's agreement; one frame, or the frames of a TID
//! without agreement (one whose agreement ended), go as single MPDUs, the
//! arena's position 0 each time. [`DataPlane::completed`] reads the arena's
//! end once. An A-MPDU's BlockAck bitmap is decoded against the arena's
//! sequence numbers in one pass over its frames: a frame it acknowledged is
//! delivered; a missing one is dropped once its MSDU's lifetime or the
//! policy's attempts on the air ran out, and retried otherwise in the same
//! arena with the same sequence and packet numbers: the arena keeps the
//! missing frames and goes again before the standby. A single MPDU had the
//! port's retry plan: delivered when acknowledged, dropped otherwise, and
//! the arena keeps the frames it did not send. A frame dropped under an
//! agreement moves the TID's window, and the recipient's is owed a
//! BlockAckReq ([`Settled::bar`]). The end also teaches the rate selection
//! ([`RateLesson`]).
//!
//! The plane reads no clock: every instant is the caller's.

use super::{
    AggregateRetry, DataProtection, ETHERNET_HEADER_LEN, MSDU_ETHERNET_OFFSET, MSDU_PAYLOAD_OFFSET,
    PpduLimit, WmmAccessCategory, WmmUserPriority, aggregate, move_down,
};
use crate::events::{AggregateSettled, DataLoss, StaEvent};
use crate::link::{TxKey, TxNumbers};
use oer_portable_events::{EventSink, report};
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::arena::{
    ARENA_FRAMES, Arena, ArenaFormat, ArenaId, ArenaPublication, AttemptEnd, CompletedOf,
    FrameMask, IntakeRefused, RetainError, TxArena,
};
use oer_protocols_ieee80211_contracts_port::{
    AggregateEnd, AggregateOutcome, Ieee80211Capabilities, IndeterminateCause, MpduProtection,
    MpduStartSpacing, PhyRate, Protection, RxMetadata, SingleEnd, SingleOutcome, SingleResponse,
    TxOutcome,
};
use oer_protocols_ieee80211_core_upper_mac::{
    ccmp::CcmpTxPacketNumber,
    qos::AcAdmission,
    rate_control::BlockAckTally,
    rate_selection::{FrameRates, RateSelection},
    sequence::{SequenceCounter, SequenceSpace},
};
use oer_protocols_ieee80211_wire::{
    ccmp::{CCMP_HEADER_LEN, CcmpHeader, CcmpKeyId},
    data::LLC_SNAP_HEADER_LEN,
    security::LinkProtection,
    sequence::SequenceNumber,
    station::{StaDataHeaderFields, StaDataHeaderTemplate},
};

/// The frame positions of one arena book.
const BOOK_FRAMES: usize = ARENA_FRAMES as usize;

/// What a protected MPDU's prefix holds after its MAC header.
const CCMP_AND_SNAP: usize = CCMP_HEADER_LEN + LLC_SNAP_HEADER_LEN;

/// One MSDU from the network: its Ethernet frame, header and payload, in
/// the network's memory at [`MSDU_ETHERNET_OFFSET`] of the arena's frame
/// region.
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
    /// The MPDU does not fit the memory's frame region.
    TooLong,
    /// The key's packet numbers are used up.
    PacketNumberExhausted,
    /// The arena refused the frame.
    Arena(IntakeRefused),
}

/// What the plane did with an MSDU.
#[derive(Debug, Eq, PartialEq)]
pub enum Intake<M> {
    /// The standby arena took it as one MPDU of this TID.
    Taken(WmmUserPriority),
    /// The standby cannot take it now: it is full, belongs to another TID
    /// or does not follow on, the MSDU's sequence number lies beyond its
    /// TID's window, or no arena is free. The MSDU, unchanged, waits in the
    /// network's queue.
    Wait(OutboundMsdu<M>),
    /// The MSDU ends: its memory goes back to the network.
    Refused {
        /// The memory it came in.
        memory: M,
        /// Why.
        reason: IntakeRefusal,
    },
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
/// an [`ArenaFormat`] ([`DataPlane::publication`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PpduPlan {
    /// The arena that goes.
    pub arena: ArenaId,
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
/// reads: a single MPDU's, or an A-MPDU's with its BlockAck tally.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateLesson<R, D> {
    /// A single MPDU of the network's data ended.
    Single(SingleOutcome<R, D>),
    /// An A-MPDU of the network's data ended.
    Aggregate {
        /// Its subframes and those the BlockAck acknowledged.
        tally: BlockAckTally,
        /// How it ended.
        outcome: AggregateOutcome<R, D>,
    },
}

impl<R: Copy, D: Copy> RateLesson<R, D> {
    /// Teach `rates` at `now`, on its rules for a current link's terminal:
    /// only an outcome with a rate sample, or an A-MPDU that went on the
    /// air, teaches it. A single MPDU's response evidence reaches the
    /// selection as its ACK evidence through `ack`; one `ack` cannot read
    /// teaches nothing.
    pub fn teach<M: RxMetadata, A, S: RateSelection<M, A, R>>(
        &self,
        now: Instant,
        rates: &mut S,
        ack: impl Fn(R) -> Option<A>,
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
fn single_with<R, A, D>(
    outcome: SingleOutcome<R, D>,
    ack: impl Fn(R) -> Option<A>,
) -> Option<SingleOutcome<A, D>> {
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
pub struct Settled<R, D> {
    /// The BlockAckReq owed: a frame was dropped under the TID's agreement,
    /// whose window moved to this head.
    pub bar: Option<(WmmUserPriority, SequenceNumber)>,
    /// What the end teaches the rate selection; `None` for an arena of an
    /// ended link.
    pub lesson: Option<RateLesson<R, D>>,
    /// Frames acknowledged.
    pub delivered: u8,
    /// Frames dropped.
    pub dropped: u8,
    /// Frames the arena keeps for its next publication.
    pub retried: u8,
}

/// What one arena's end of the backend `A` settled.
pub type SettledOf<'p, 'id, A> =
    Settled<<A as TxArena<'p, 'id>>::Response, <A as TxArena<'p, 'id>>::Denial>;

/// A completion of the backend `A` the plane did not settle.
pub type SettleErrorOf<'p, 'id, A> =
    SettleError<CompletedOf<'p, 'id, A>, <A as TxArena<'p, 'id>>::Fault>;

/// A completion the plane did not settle.
#[derive(Debug)]
pub enum SettleError<C, F> {
    /// The completion names an arena the plane did not publish, or holds
    /// another number of frames than it published: it comes back unsettled.
    Unexpected(C),
    /// The backend faulted while settling it.
    Fault(F),
}

/// What the plane knows of the frames one arena holds. Frames are named by
/// their original position, the order intake took them in: position `o`
/// carries the sequence number `first + o`. The arena holds the positions
/// of `pending` in their order; a retry compacts the arena, not the book.
#[derive(Debug)]
struct ArenaBook {
    /// The arena's right while the plane holds it: `None` while published.
    right: Option<Arena>,
    /// The link generation that filled it.
    epoch: u32,
    tid: WmmUserPriority,
    access_category: WmmAccessCategory,
    /// The sequence number of original position 0.
    first: SequenceNumber,
    /// Original positions taken.
    taken: u8,
    /// Original positions the arena still holds.
    pending: u64,
    /// The subframe octets of the taken frames.
    octets: u32,
    /// When each original position's MSDU was admitted.
    admitted: [Instant; BOOK_FRAMES],
    /// Publications that may have reached a receiver: every frame the arena
    /// holds went in each.
    attempts: u8,
    /// The arena holds frames to retry and takes no more.
    retained: bool,
    /// How the publication on the air goes.
    published: Option<PpduKind>,
}

impl ArenaBook {
    const fn new(right: Arena) -> Self {
        Self {
            right: Some(right),
            epoch: 0,
            tid: WmmUserPriority::UP0,
            access_category: WmmAccessCategory::BestEffort,
            first: SequenceNumber::from_low_bits(0),
            taken: 0,
            pending: 0,
            octets: 0,
            admitted: [Instant::EPOCH; BOOK_FRAMES],
            attempts: 0,
            retained: false,
            published: None,
        }
    }

    /// Frames the arena holds.
    const fn count(&self) -> u8 {
        // At most 64 bits are set.
        self.pending.count_ones() as u8
    }

    /// The arena holds no frame: its book starts again.
    const fn clear(&mut self) {
        self.taken = 0;
        self.pending = 0;
        self.octets = 0;
        self.attempts = 0;
        self.retained = false;
    }

    /// The arena is the standby: the plane holds it and it takes frames.
    const fn takes(&self) -> bool {
        self.right.is_some() && !self.retained
    }

    /// The oldest sequence number the arena holds of `tid` for the link
    /// generation `epoch`.
    fn oldest(&self, epoch: u32, tid: WmmUserPriority) -> Option<SequenceNumber> {
        if self.epoch != epoch || self.tid != tid || self.pending == 0 {
            return None;
        }
        let offset = u16::try_from(self.pending.trailing_zeros()).ok()?;
        Some(self.first.wrapping_add(offset))
    }
}

/// The connected station's data plane over a backend's two arenas.
pub struct DataPlane {
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
    first: ArenaBook,
    second: ArenaBook,
}

impl DataPlane {
    /// The plane over the backend's two arena rights, as
    /// [`TxArena::arenas`] mints them.
    ///
    /// # Errors
    ///
    /// The rights, unless they are the first and the second arena in order.
    pub fn new(arenas: [Arena; 2]) -> Result<Self, [Arena; 2]> {
        let [first, second] = arenas;
        if first.id() != ArenaId::First || second.id() != ArenaId::Second {
            return Err([first, second]);
        }
        Ok(Self {
            link: None,
            epoch: 0,
            rate: None,
            limits: [None; 8],
            next: [None; 8],
            first: ArenaBook::new(first),
            second: ArenaBook::new(second),
        })
    }

    const fn book(&self, id: ArenaId) -> &ArenaBook {
        match id {
            ArenaId::First => &self.first,
            ArenaId::Second => &self.second,
        }
    }

    const fn book_mut(&mut self, id: ArenaId) -> &mut ArenaBook {
        match id {
            ArenaId::First => &mut self.first,
            ArenaId::Second => &mut self.second,
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
        if link.header.peer_qos() {
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
    /// network through `returned`, counted, and the link's data state comes
    /// back. The arena on the air settles at its end, its frames released.
    ///
    /// # Errors
    ///
    /// A backend fault; the plane keeps the link.
    pub fn stop<'p, 'id: 'p, A: TxArena<'p, 'id>>(
        &mut self,
        arena: &mut A,
        mut returned: impl FnMut(A::Memory),
        events: &mut impl EventSink<StaEvent>,
    ) -> Result<Option<DataLink>, A::Fault> {
        for id in [ArenaId::First, ArenaId::Second] {
            self.withdraw(arena, id, &mut returned, events)?;
        }
        self.rate = None;
        self.limits = [None; 8];
        self.next = [None; 8];
        Ok(self.link.take())
    }

    /// Return every frame the arenas hold unpublished, keeping the link:
    /// the frames written under the link's key before a new one replaces it
    /// carry that key's packet numbers.
    ///
    /// # Errors
    ///
    /// A backend fault.
    pub fn withdraw_unpublished<'p, 'id: 'p, A: TxArena<'p, 'id>>(
        &mut self,
        arena: &mut A,
        mut returned: impl FnMut(A::Memory),
        events: &mut impl EventSink<StaEvent>,
    ) -> Result<(), A::Fault> {
        for id in [ArenaId::First, ArenaId::Second] {
            self.withdraw(arena, id, &mut returned, events)?;
        }
        Ok(())
    }

    /// Whether an arena is on the air: its completion is owed.
    pub fn on_air(&self) -> bool {
        self.first.right.is_none() || self.second.right.is_none()
    }

    /// The frames the arenas hold: published, retained or taken.
    pub fn frames(&self) -> u8 {
        self.first.count().saturating_add(self.second.count())
    }

    /// Whether an arena holds a frame: published, retained or taken.
    pub fn holds_frames(&self) -> bool {
        self.first.pending != 0 || self.second.pending != 0
    }

    /// Return every frame the unpublished arena `id` holds.
    fn withdraw<'p, 'id: 'p, A: TxArena<'p, 'id>>(
        &mut self,
        arena: &mut A,
        id: ArenaId,
        returned: &mut impl FnMut(A::Memory),
        events: &mut impl EventSink<StaEvent>,
    ) -> Result<(), A::Fault> {
        let book = self.book_mut(id);
        let Some(right) = book.right.as_ref() else {
            return Ok(());
        };
        if book.pending == 0 {
            return Ok(());
        }
        arena.withdraw(right, |memory| {
            report(events, || StaEvent::DataLost(DataLoss::LeftBehind));
            returned(memory);
        })?;
        book.clear();
        Ok(())
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
    /// agreement goes as single MPDUs, one position at a time.
    pub fn agreement(&mut self, tid: WmmUserPriority, window: Option<TxWindow>) {
        let Some(link) = self.link.as_mut() else {
            return;
        };
        *tid.select_mut(&mut link.windows) = window;
        self.limit(tid);
    }

    /// The arena the next frame goes to: the one filled, else an empty one
    /// the plane holds.
    fn standby(&self) -> Option<ArenaId> {
        let ids = [ArenaId::First, ArenaId::Second];
        ids.into_iter()
            .find(|id| self.book(*id).takes() && self.book(*id).pending != 0)
            .or_else(|| ids.into_iter().find(|id| self.book(*id).takes()))
    }

    /// Take `msdu`, admitted at `now`, into the standby arena as one MPDU:
    /// its header is written in front of its payload from the link's
    /// template, with the next sequence number of its TID in `numbers` and,
    /// on a protected link, the CCMP header of the key's next packet number.
    /// No sequence number is drawn for an MSDU that waits or is refused, and
    /// no packet number before the arena's writer runs.
    pub fn intake<'p, 'id: 'p, A: TxArena<'p, 'id>, P: TxKey>(
        &mut self,
        arena: &mut A,
        msdu: OutboundMsdu<A::Memory>,
        numbers: &mut TxNumbers<P>,
        now: Instant,
    ) -> Intake<A::Memory> {
        let refused = |msdu: OutboundMsdu<A::Memory>, reason| Intake::Refused {
            memory: msdu.memory,
            reason,
        };
        let Some(link) = self.link.as_ref() else {
            return refused(msdu, IntakeRefusal::NoLink);
        };
        let header = link.header;
        // The final priority names the frame's TID, its sequence space and
        // its access category; a non-QoS link has one queue, best effort.
        let (tid, access_category) = if header.peer_qos() {
            match link.admission.admitted_user_priority(msdu.priority) {
                Some(tid) => (tid, tid.access_category()),
                None => return refused(msdu, IntakeRefusal::NoAdmittedAccessCategory),
            }
        } else {
            (WmmUserPriority::UP0, WmmAccessCategory::BestEffort)
        };
        let protection = match header.protection() {
            LinkProtection::Open => DataProtection::Open,
            LinkProtection::Ccmp => DataProtection::Ccmp(()),
        };
        if header.peer_qos() != numbers.qos() {
            return refused(msdu, IntakeRefusal::Numbering);
        }
        if protection == DataProtection::Ccmp(()) && numbers.key().is_none() {
            return refused(msdu, IntakeRefusal::NoKey);
        }
        let Some(payload) = msdu.length.checked_sub(ETHERNET_HEADER_LEN) else {
            return refused(msdu, IntakeRefusal::ShortFrame);
        };
        let Some(octets) = header
            .prefix_len()
            .checked_add(payload)
            .and_then(|mpdu| aggregate::subframe_octets(mpdu, protection))
        else {
            return refused(msdu, IntakeRefusal::TooLong);
        };
        let sequence = numbers.next(tid);
        let window = *tid.select(&link.windows);
        let Some(id) = self.standby() else {
            return Intake::Wait(msdu);
        };
        if window.is_some_and(|window| window.ahead(sequence)) {
            return Intake::Wait(msdu);
        }
        let book = self.book(id);
        let count = book.count();
        if count != 0 {
            let limit = *tid.select(&self.limits);
            let follows =
                book.tid == tid && u16::from(book.taken) == book.first.forward_distance(sequence);
            let fits = limit.is_some_and(|limit| {
                usize::from(count) < limit.subframes()
                    && count < arena.capacity()
                    && book
                        .octets
                        .checked_add(octets)
                        .is_some_and(|total| total <= limit.octets())
            });
            if !follows || !fits {
                return Intake::Wait(msdu);
            }
        }
        let epoch = self.epoch;
        let Self {
            first,
            second,
            next,
            ..
        } = self;
        let book = match id {
            ArenaId::First => first,
            ArenaId::Second => second,
        };
        let Some(right) = book.right.as_ref() else {
            return Intake::Wait(msdu);
        };
        let (counter, key) = numbers.data_and_key(tid);
        let packet_numbers = key.map(TxKey::packet_numbers);
        let saved = *counter;
        let mut written = None;
        let taken = arena.intake(right, msdu.memory, |region| {
            let result = write_mpdu(
                region,
                &header,
                Fields {
                    priority: tid,
                    payload,
                },
                counter,
                packet_numbers,
            );
            let length = result.ok();
            written = Some(result);
            length
        });
        match taken {
            Ok(_index) => {
                if count == 0 {
                    book.epoch = epoch;
                    book.tid = tid;
                    book.access_category = access_category;
                    book.first = sequence;
                    book.clear();
                }
                let position = book.taken;
                if let Some(admitted) = book.admitted.get_mut(usize::from(position)) {
                    *admitted = now;
                }
                book.pending |= 1_u64.checked_shl(u32::from(position)).unwrap_or(0);
                book.taken = position.saturating_add(1);
                book.octets = book.octets.saturating_add(octets);
                *tid.select_mut(next) = Some(sequence.wrapping_add(1));
                Intake::Taken(tid)
            }
            Err((memory, refusal)) => {
                // A frame the arena did not keep took no sequence number; a
                // packet number its writer drew leaves a gap, never a repeat.
                *counter = saved;
                let reason = match (refusal, written) {
                    (IntakeRefused::Declined, Some(Err(reason))) => reason,
                    (refusal, _) => IntakeRefusal::Arena(refusal),
                };
                Intake::Refused { memory, reason }
            }
        }
    }

    /// Plan the next publication, at the rate `rate` gives the plan: the
    /// arena whose frames wait from an earlier publication first, then the
    /// standby; more frames than one as an A-MPDU under the TID's
    /// agreement, else a single MPDU of position 0. `None` while an arena is
    /// on the air, none holds a frame, or `rate` gives no format of the
    /// plan's kind.
    pub fn publication<'p, 'id, K: Ieee80211Capabilities>(
        &mut self,
        rate: impl FnOnce(PpduPlan) -> Option<ArenaFormat<'p, 'id, K>>,
    ) -> Option<ArenaPublication<'p, 'id, K>> {
        let link = self.link.as_ref()?;
        let id = self.publishable()?;
        let book = self.book(id);
        let frames = book.count();
        // More frames than one of a TID without agreement go as single
        // MPDUs, position 0 each time.
        let kind = match link.ht {
            Some(ht) if frames > 1 && self.aggregates(book.tid) => PpduKind::Aggregate {
                spacing: ht.spacing,
            },
            _ => PpduKind::Single,
        };
        let encryption = match link.header.protection() {
            LinkProtection::Open => MpduProtection::Open,
            LinkProtection::Ccmp => MpduProtection::Ccmp,
        };
        let format = rate(PpduPlan {
            arena: id,
            tid: book.tid,
            frames,
            kind,
        })?;
        let matches = matches!(
            (&format, kind),
            (ArenaFormat::Single(_), PpduKind::Single)
                | (ArenaFormat::Aggregate { .. }, PpduKind::Aggregate { .. })
        );
        if !matches {
            return None;
        }
        let access_category = book.access_category;
        let book = self.book_mut(id);
        let arena = book.right.take()?;
        book.published = Some(kind);
        Some(ArenaPublication {
            arena,
            format,
            access_category,
            protection: Protection::None,
            encryption,
            power: crate::machine::calibrated_power::<K>(),
        })
    }

    /// The backend refused `publication`: its arena's right comes back, and
    /// the arena goes again at the next publication.
    pub fn refused<K>(&mut self, publication: ArenaPublication<'_, '_, K>) {
        let book = self.book_mut(publication.arena.id());
        if book.right.is_none() {
            book.published = None;
            book.right = Some(publication.arena);
        }
    }

    /// Settle the completed arena `done` at `now`: acknowledged frames are
    /// delivered; an A-MPDU's missing ones are retried in the same arena
    /// while their MSDU's lifetime and the policy's attempts last, dropped
    /// after; a single MPDU's unacknowledged frame is dropped and the frames
    /// it did not send stay for the next publication; every memory
    /// that leaves the arena goes back to the network through `returned`,
    /// in position order. An arena of an ended link releases every frame.
    ///
    /// # Errors
    ///
    /// The completion, unsettled, when it is not the published arena's; a
    /// backend fault.
    pub fn completed<'p, 'id: 'p, A>(
        &mut self,
        arena: &mut A,
        done: CompletedOf<'p, 'id, A>,
        now: Instant,
        returned: impl FnMut(A::Memory),
        events: &mut impl EventSink<StaEvent>,
    ) -> Result<SettledOf<'p, 'id, A>, SettleErrorOf<'p, 'id, A>>
    where
        A: TxArena<'p, 'id, Response: Copy, Denial: Copy>,
    {
        let id = done.id();
        let book = self.book(id);
        let (Some(kind), None) = (book.published, book.right.as_ref()) else {
            return Err(SettleError::Unexpected(done));
        };
        if done.frames() != book.count() {
            return Err(SettleError::Unexpected(done));
        }
        let tid = book.tid;
        let link = self.link.as_ref().filter(|_| book.epoch == self.epoch);
        let Some(link) = link else {
            // An ended link's frames neither retry nor move its windows.
            let right = arena.release(done, returned).map_err(SettleError::Fault)?;
            let book = self.book_mut(id);
            book.right = Some(right);
            book.published = None;
            book.clear();
            return Ok(Settled {
                bar: None,
                lesson: None,
                delivered: 0,
                dropped: 0,
                retried: 0,
            });
        };
        let end = done.end();
        let (decision, attempts) = match kind {
            PpduKind::Single => (Self::single_end(book, end, events), book.attempts),
            PpduKind::Aggregate { .. } => {
                let attempts = book.attempts.saturating_add(u8::from(end.air().is_some()));
                let decision = Self::aggregate_end(book, link.retry, attempts, end, now, events);
                (decision, attempts)
            }
        };
        let lesson = lesson(end, kind, done.frames(), decision.delivered);
        let rate = end.air().map(|air| air.rate);
        let frames = done.frames();
        let retried = decision.retry.count();
        let right = if decision.retry.is_empty() {
            arena.release(done, returned).map_err(SettleError::Fault)?
        } else {
            arena
                .retain(done, decision.retry, returned)
                .map_err(|error| match error {
                    RetainError::Outside(done) => SettleError::Unexpected(done),
                    RetainError::Fault(fault) => SettleError::Fault(fault),
                })?
        };
        let book = self.book_mut(id);
        book.right = Some(right);
        book.published = None;
        if decision.pending == 0 {
            book.clear();
        } else {
            book.pending = decision.pending;
            book.attempts = attempts;
            book.retained = true;
        }
        if matches!(kind, PpduKind::Aggregate { .. }) {
            report(events, || {
                StaEvent::AggregateSettled(AggregateSettled {
                    members: frames,
                    acknowledged: decision.delivered,
                    retried,
                    dropped: decision.dropped,
                    refused: false,
                    rate,
                })
            });
        }
        let head = self.advance(tid);
        Ok(Settled {
            bar: head
                .filter(|_| decision.dropped != 0)
                .map(|head| (tid, head)),
            lesson,
            delivered: decision.delivered,
            dropped: decision.dropped,
            retried,
        })
    }

    /// A single MPDU's end: position 0 is delivered when acknowledged and
    /// dropped otherwise, once the port's retry plan ran out; the frames
    /// the publication did not send stay for the next.
    fn single_end<R, D>(
        book: &ArenaBook,
        end: &AttemptEnd<R, D>,
        events: &mut impl EventSink<StaEvent>,
    ) -> Decision {
        let mut decision = Decision::default();
        let acknowledged = match end {
            AttemptEnd::Acked { .. } => true,
            AttemptEnd::Answered { report, .. } => {
                book.oldest(book.epoch, book.tid).is_some_and(|sequence| {
                    oer_protocols_ieee80211_core_upper_mac::block_ack::acknowledges(
                        *report, sequence,
                    )
                })
            }
            _ => false,
        };
        if acknowledged {
            decision.delivered = 1;
        } else {
            let loss = match end {
                AttemptEnd::Aborted => DataLoss::Indeterminate,
                AttemptEnd::Collision(_) => DataLoss::ContentionLimited,
                _ => DataLoss::RetryLimit,
            };
            report(events, || StaEvent::DataLost(loss));
            decision.dropped = 1;
        }
        // The positions after 0, compacted by the retention.
        decision.pending = book.pending & book.pending.wrapping_sub(1);
        let count = book.count();
        decision.retry = FrameMask::first(count)
            .zip(FrameMask::first(1))
            .map_or(FrameMask::EMPTY, |(all, head)| all.difference(head));
        decision
    }

    /// An A-MPDU's end after `attempts` publications that may have reached
    /// a receiver: each frame the BlockAck acknowledges is delivered, a
    /// missing one dropped once its MSDU's lifetime or the attempts ran out,
    /// kept otherwise. One pass over the frames.
    fn aggregate_end<R, D>(
        book: &ArenaBook,
        retry: Option<AggregateRetry>,
        attempts: u8,
        end: &AttemptEnd<R, D>,
        now: Instant,
        events: &mut impl EventSink<StaEvent>,
    ) -> Decision {
        let mut decision = Decision::default();
        let positions = FrameMask::first(book.count()).unwrap_or(FrameMask::EMPTY);
        for (position, original) in positions.iter().zip(Originals(book.pending)) {
            let sequence = book
                .first
                .wrapping_add(u16::try_from(original).unwrap_or(u16::MAX));
            let acknowledged = match end {
                AttemptEnd::Acked { .. } => true,
                AttemptEnd::Answered { report, .. } => {
                    oer_protocols_ieee80211_core_upper_mac::block_ack::acknowledges(
                        *report, sequence,
                    )
                }
                _ => false,
            };
            if acknowledged {
                decision.delivered = decision.delivered.saturating_add(1);
                continue;
            }
            let admitted = usize::try_from(original)
                .ok()
                .and_then(|original| book.admitted.get(original))
                .copied()
                .unwrap_or(Instant::EPOCH);
            let loss = if matches!(end, AttemptEnd::Aborted) {
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
                    report(events, || StaEvent::DataLost(loss));
                    decision.dropped = decision.dropped.saturating_add(1);
                }
                None => {
                    decision.retry.insert(position);
                    decision.pending |= 1_u64.checked_shl(original).unwrap_or(0);
                }
            }
        }
        decision
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

    /// The arena the next publication puts on the air: none while one is on
    /// the air, else the one whose frames missed their BlockAck, else the
    /// standby when it holds a frame.
    pub fn publishable(&self) -> Option<ArenaId> {
        let ids = [ArenaId::First, ArenaId::Second];
        if ids.iter().any(|id| self.book(*id).right.is_none()) {
            return None;
        }
        ids.into_iter()
            .find(|id| self.book(*id).pending != 0 && self.book(*id).retained)
            .or_else(|| ids.into_iter().find(|id| self.book(*id).pending != 0))
    }
}

/// What one end decides of an arena's frames.
#[derive(Default)]
struct Decision {
    delivered: u8,
    dropped: u8,
    /// Arena positions kept for the next publication.
    retry: FrameMask,
    /// Their original positions.
    pending: u64,
}

/// The original positions of a book's `pending` mask, in ascending order:
/// the order the arena holds their frames in.
struct Originals(u64);

impl Iterator for Originals {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        if self.0 == 0 {
            return None;
        }
        let original = self.0.trailing_zeros();
        self.0 &= self.0.wrapping_sub(1);
        Some(original)
    }
}

/// The lesson of an arena's end, `frames` of which went and `delivered`
/// were acknowledged. An A-MPDU answered by an ACK has no BlockAck to teach.
fn lesson<R: Copy, D: Copy>(
    end: &AttemptEnd<R, D>,
    kind: PpduKind,
    frames: u8,
    delivered: u8,
) -> Option<RateLesson<R, D>> {
    match kind {
        PpduKind::Single => {
            let outcome = match *end {
                AttemptEnd::Acked { air, response } => TxOutcome::Ended(SingleEnd::Delivered {
                    air,
                    response: SingleResponse::Ack(response),
                }),
                AttemptEnd::Answered {
                    air,
                    report,
                    response,
                } => TxOutcome::Ended(SingleEnd::Delivered {
                    air,
                    response: SingleResponse::BlockAck {
                        report,
                        ack: response,
                    },
                }),
                AttemptEnd::NoResponse(air) => TxOutcome::Ended(SingleEnd::AckTimeout(air)),
                AttemptEnd::ProtectionFailure(air) => TxOutcome::Ended(SingleEnd::CtsTimeout(air)),
                AttemptEnd::Collision(air) => TxOutcome::Ended(SingleEnd::ContentionLimit(air)),
                AttemptEnd::AirDenied { denial, air } => {
                    TxOutcome::Ended(SingleEnd::AirDenied { denial, air })
                }
                AttemptEnd::Aborted => {
                    TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)
                }
            };
            Some(RateLesson::Single(outcome))
        }
        PpduKind::Aggregate { .. } => {
            let outcome = match *end {
                AttemptEnd::Answered {
                    air,
                    report,
                    response,
                } => TxOutcome::Ended(AggregateEnd::Delivered {
                    air,
                    report,
                    ack: response,
                }),
                AttemptEnd::Acked { .. } => return None,
                AttemptEnd::NoResponse(air) => TxOutcome::Ended(AggregateEnd::AckTimeout(air)),
                AttemptEnd::ProtectionFailure(air) => {
                    TxOutcome::Ended(AggregateEnd::CtsTimeout(air))
                }
                AttemptEnd::Collision(air) => TxOutcome::Ended(AggregateEnd::ContentionLimit(air)),
                AttemptEnd::AirDenied { denial, air } => {
                    TxOutcome::Ended(AggregateEnd::AirDenied { denial, air })
                }
                AttemptEnd::Aborted => {
                    TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)
                }
            };
            let attempted = core::num::NonZeroU16::new(u16::from(frames))?;
            let tally = BlockAckTally::new(attempted, u16::from(delivered)).ok()?;
            Some(RateLesson::Aggregate { tally, outcome })
        }
    }
}

/// What the header write knows of the MSDU: the frame's TID and its
/// payload's length at [`MSDU_PAYLOAD_OFFSET`].
struct Fields {
    priority: WmmUserPriority,
    payload: usize,
}

/// Write the MPDU of the MSDU whose Ethernet frame sits at
/// [`MSDU_ETHERNET_OFFSET`] of `region`, from the region's first octet:
/// `header`'s prefix in front of the payload, which a shorter prefix moves
/// down to it, with the next number of `sequence` and, on a protected link,
/// the CCMP header of the next packet number of `numbers`. Its length, without FCS and
/// MIC. Nothing is written or drawn for an MPDU that does not fit.
fn write_mpdu(
    region: &mut [u8],
    header: &StaDataHeaderTemplate,
    msdu: Fields,
    sequence: &mut SequenceCounter,
    numbers: Option<&CcmpTxPacketNumber>,
) -> Result<u16, IntakeRefusal> {
    let ethernet = region
        .get(MSDU_ETHERNET_OFFSET..MSDU_PAYLOAD_OFFSET)
        .ok_or(IntakeRefusal::TooLong)?;
    let (destination, rest) = ethernet
        .split_first_chunk::<6>()
        .ok_or(IntakeRefusal::TooLong)?;
    let destination = *destination;
    let ether_type = u16::from_be_bytes(*rest.last_chunk::<2>().ok_or(IntakeRefusal::TooLong)?);
    let prefix = header.prefix_len();
    let end = MSDU_PAYLOAD_OFFSET
        .checked_add(msdu.payload)
        .filter(|end| *end <= region.len())
        .ok_or(IntakeRefusal::TooLong)?;
    let length = prefix
        .checked_add(msdu.payload)
        .and_then(|length| u16::try_from(length).ok())
        .ok_or(IntakeRefusal::TooLong)?;
    let ccmp = match (header.protection(), numbers) {
        (LinkProtection::Open, _) => None,
        (LinkProtection::Ccmp, None) => return Err(IntakeRefusal::NoKey),
        (LinkProtection::Ccmp, Some(numbers)) => {
            let packet_number = numbers
                .allocate()
                .map_err(|_| IntakeRefusal::PacketNumberExhausted)?;
            Some(CcmpHeader::new(packet_number, CcmpKeyId::PAIRWISE).encode())
        }
    };
    let fields = StaDataHeaderFields {
        destination,
        sequence_number: sequence.take(),
        priority: msdu.priority,
        ether_type,
    };
    if prefix < MSDU_PAYLOAD_OFFSET {
        move_down(region, prefix, end).ok_or(IntakeRefusal::TooLong)?;
    }
    header
        .write(fields, region)
        .map_err(|_| IntakeRefusal::TooLong)?;
    if let Some(ccmp) = ccmp {
        // The CCMP header follows the MAC header, before LLC/SNAP.
        let at = prefix
            .checked_sub(CCMP_AND_SNAP)
            .ok_or(IntakeRefusal::TooLong)?;
        let place = region
            .get_mut(at..)
            .and_then(<[u8]>::first_chunk_mut::<CCMP_HEADER_LEN>)
            .ok_or(IntakeRefusal::TooLong)?;
        *place = ccmp;
    }
    Ok(length)
}

#[cfg(test)]
mod tests;
