//! Data transmission on two DMA arenas.
//!
//! The backend owns two arenas of transmit slots ([`ArenaId`]). The caller
//! holds one affine [`Arena`] right per arena: with it the caller fills the
//! arena frame by frame ([`TxArena::intake`]), each frame written once into
//! the memory the caller hands over, then publishes the arena as one PPDU
//! ([`TxArena::publish`]): an A-MPDU of every frame, or a single MPDU of the
//! frame at position 0 while the others wait in the arena. Publication moves the
//! right into the backend; one arena is on the air at a time while the other
//! is filled. [`TxArena::service`] reports the arena's end as an affine
//! [`ArenaCompleted`], which carries the right back, and only consuming it
//! through [`TxArena::retain`] or [`TxArena::release`] returns the right: a
//! completed arena is settled before it is filled or published again.
//!
//! Frames are positions `0..len` of their arena. [`FrameIndex`] names one
//! position and [`FrameMask`] a set of them, bit `i` for position `i`.
//! Retaining keeps the selected frames in their arena in their relative
//! order, compacted to positions `0..k`, and returns every other memory in
//! position order; releasing returns every memory in position order. A
//! caller drains an arena as single MPDUs by keeping every position but 0
//! after each.
//! Sequence numbers, packet numbers, BlockAck windows, ageing and the retry
//! policy belong to the caller, which writes each frame's header at intake;
//! descriptors, the Retry bit of a retransmission, the MAC program and
//! completion detection belong to the backend.
//!
//! The backend implements the required methods. The provided methods are
//! the caller's operations: they alone mint arena rights, frame indices and
//! completions, and they alone settle a completion, so a caller can neither
//! forge an arena nor publish one that awaits settlement:
//!
//! ```compile_fail,E0616
//! use oer_protocols_ieee80211_contracts_port::arena::{Arena, ArenaCompleted};
//! fn unsettled<R, D>(done: ArenaCompleted<R, D>) -> Arena {
//!     done.arena
//! }
//! ```

use crate::{
    Air, AmpduRate, BlockAckReport, Ieee80211Capabilities, MpduProtection, MpduStartSpacing,
    PhyRate, PortRate, Protection, RetryPlan, SupportedPower,
};
use oer_portable_time::Instant;
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;

/// Number of frame positions carried by the port's `u32` retry/aging masks.
pub const AMPDU_SUBFRAME_MASK_BITS: usize = u32::BITS as usize;

/// Frame positions of one arena: the width of the port's A-MPDU subframe
/// masks, at most the 64 positions of [`FrameMask`].
pub const ARENA_FRAMES: u8 = {
    if AMPDU_SUBFRAME_MASK_BITS > u64::BITS as usize {
        panic!("arena positions fit the u64 frame mask");
    }
    AMPDU_SUBFRAME_MASK_BITS as u8
};

/// One frame position of an arena, below [`ARENA_FRAMES`]. Intake mints it
/// for the frame it adds; a [`FrameMask`] yields the positions it holds.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FrameIndex(u8);

impl FrameIndex {
    const fn checked(position: u8) -> Option<Self> {
        if position < ARENA_FRAMES {
            Some(Self(position))
        } else {
            None
        }
    }

    /// The position.
    pub const fn get(self) -> u8 {
        self.0
    }

    const fn bit(self) -> u64 {
        match 1_u64.checked_shl(self.0 as u32) {
            Some(bit) => bit,
            None => 0,
        }
    }
}

/// A set of frame positions of one arena: bit `i` is position `i`. No bit
/// at or above [`ARENA_FRAMES`] is ever set.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct FrameMask(u64);

impl FrameMask {
    /// No position.
    pub const EMPTY: Self = Self(0);

    /// Every position below [`ARENA_FRAMES`].
    const VALID: u64 = match 1_u64.checked_shl(ARENA_FRAMES as u32) {
        Some(bit) => bit.wrapping_sub(1),
        None => u64::MAX,
    };

    /// The positions of `bits`, when none is at or above [`ARENA_FRAMES`].
    pub const fn from_bits(bits: u64) -> Option<Self> {
        if bits & !Self::VALID == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// Positions `0..count`, when `count` is at most [`ARENA_FRAMES`].
    pub const fn first(count: u8) -> Option<Self> {
        if count > ARENA_FRAMES {
            return None;
        }
        match 1_u64.checked_shl(count as u32) {
            Some(bit) => Some(Self(bit.wrapping_sub(1))),
            None => Some(Self(u64::MAX)),
        }
    }

    /// The mask's bits.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Whether the mask holds `index`.
    pub const fn contains(self, index: FrameIndex) -> bool {
        self.0 & index.bit() != 0
    }

    /// The mask with `index` added.
    #[must_use]
    pub const fn with(self, index: FrameIndex) -> Self {
        Self(self.0 | index.bit())
    }

    /// The mask without `index`.
    #[must_use]
    pub const fn without(self, index: FrameIndex) -> Self {
        Self(self.0 & !index.bit())
    }

    /// Add `index`.
    pub const fn insert(&mut self, index: FrameIndex) {
        self.0 |= index.bit();
    }

    /// Remove `index`.
    pub const fn remove(&mut self, index: FrameIndex) {
        self.0 &= !index.bit();
    }

    /// The positions of either mask.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The positions of both masks.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// The positions of this mask that `other` lacks.
    #[must_use]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// The number of positions.
    pub const fn count(self) -> u8 {
        // At most 64 bits are set, so the count fits.
        self.0.count_ones() as u8
    }

    /// Whether the mask holds no position.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether every position is below `frames`.
    pub const fn is_within(self, frames: u8) -> bool {
        match Self::first(frames) {
            Some(first) => self.0 & !first.0 == 0,
            None => true,
        }
    }

    /// The positions, in ascending order.
    pub const fn iter(self) -> FrameIter {
        FrameIter(self.0)
    }
}

impl IntoIterator for FrameMask {
    type Item = FrameIndex;
    type IntoIter = FrameIter;
    fn into_iter(self) -> FrameIter {
        self.iter()
    }
}

/// The positions of a [`FrameMask`], in ascending order.
#[derive(Clone, Debug)]
pub struct FrameIter(u64);

impl Iterator for FrameIter {
    type Item = FrameIndex;
    fn next(&mut self) -> Option<FrameIndex> {
        if self.0 == 0 {
            return None;
        }
        let position = u8::try_from(self.0.trailing_zeros()).ok()?;
        self.0 &= self.0.wrapping_sub(1);
        FrameIndex::checked(position)
    }
}

/// One of the backend's two arenas.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ArenaId {
    /// The first arena.
    First,
    /// The second arena.
    Second,
}

impl ArenaId {
    /// The other arena.
    pub const fn other(self) -> Self {
        match self {
            Self::First => Self::Second,
            Self::Second => Self::First,
        }
    }

    /// The arena's place in a backend's pair of arenas: 0 or 1.
    pub const fn index(self) -> usize {
        match self {
            Self::First => 0,
            Self::Second => 1,
        }
    }
}

/// The caller's affine right to one arena: to fill it, publish it and
/// withdraw its frames. [`TxArena::arenas`] mints the backend's two rights
/// once; publication moves a right into the backend and settlement of its
/// completion returns it.
#[must_use = "an arena right is the only way to fill, publish or withdraw its arena"]
#[derive(Debug, Eq, PartialEq)]
pub struct Arena(ArenaId);

impl Arena {
    /// The arena this right names.
    pub const fn id(&self) -> ArenaId {
        self.0
    }
}

/// How an arena goes on the air, at a rate the port checked.
pub enum ArenaFormat<'p, K> {
    /// One MPDU that solicits an ACK: the frame at position 0, sent by the
    /// retry plan as the backend sends any single MPDU (`RetryPlan`): it
    /// keeps the frame's sequence number, sets the Retry bit of each
    /// retransmission and ends at the first ACK or when the plan runs out.
    /// The arena's other frames stay unsent in it, and its completion
    /// reports every frame it holds and the plan's transmissions.
    Single(RetryPlan<PortRate<'p, K>>),
    /// An A-MPDU that solicits a BlockAck.
    Aggregate {
        /// A rate the backend aggregates at.
        rate: AmpduRate<'p, K>,
        /// The recipient's minimum MPDU start spacing.
        spacing: MpduStartSpacing,
    },
}

impl<K> Clone for ArenaFormat<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K> Copy for ArenaFormat<'_, K> {}

impl<K> core::fmt::Debug for ArenaFormat<'_, K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Single(plan) => f.debug_tuple("Single").field(&plan.first().rate).finish(),
            Self::Aggregate { rate, spacing } => f
                .debug_struct("Aggregate")
                .field("rate", rate.rate())
                .field("spacing", spacing)
                .finish(),
        }
    }
}

impl<K> ArenaFormat<'_, K> {
    /// The rate the PPDU goes at first.
    pub const fn rate(&self) -> PhyRate {
        match self {
            Self::Single(plan) => plan.first().rate.get(),
            Self::Aggregate { rate, .. } => rate.rate().get(),
        }
    }
}

/// One publication of an arena: its right, its format and how it goes.
pub struct ArenaPublication<'p, K> {
    /// The arena's right, which the backend holds until the completion.
    pub arena: Arena,
    /// Single MPDU or A-MPDU, and its rate.
    pub format: ArenaFormat<'p, K>,
    /// The EDCA queue the PPDU contends on.
    pub access_category: WmmAccessCategory,
    /// The control exchange that protects the PPDU.
    pub protection: Protection,
    /// Whether the hardware encrypts every frame under the interface's
    /// installed pairwise key: the caller wrote each frame's CCMP header.
    pub encryption: MpduProtection,
    /// The PPDU's transmit power.
    pub power: SupportedPower<K>,
}

impl<K: Ieee80211Capabilities> core::fmt::Debug for ArenaPublication<'_, K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ArenaPublication")
            .field("arena", &self.arena)
            .field("format", &self.format)
            .field("access_category", &self.access_category)
            .field("protection", &self.protection)
            .field("encryption", &self.encryption)
            .field("power", &self.power.get())
            .finish()
    }
}

/// Why intake refused a frame; the memory comes back with it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntakeRefused {
    /// The arena holds as many frames as it has slots.
    Full,
    /// The writer wrote no frame.
    Declined,
    /// The written length is zero or exceeds the memory's frame region.
    Length,
    /// The arena is published or awaits settlement in the backend.
    Busy,
    /// The backend is quarantined after a fault.
    Quarantined,
}

/// Why the backend refused a publication; the publication comes back with it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishRefused {
    /// The arena holds no frame.
    Empty,
    /// The other arena is on the air, or this one is not idle.
    Busy,
    /// Encryption without an installed pairwise key.
    NoKey,
    /// A frame the publication sends carries a Protected bit its
    /// encryption contradicts.
    Protection,
    /// The A-MPDU exceeds the backend's hardware octet or subframe limit at
    /// its rate; the backend checks it, never trusting the caller.
    Length,
}

/// A failed publication.
pub enum PublishError<'p, K, F> {
    /// The backend refused it before any hardware change.
    Refused {
        /// The unchanged publication, with its arena right.
        publication: ArenaPublication<'p, K>,
        /// Why.
        reason: PublishRefused,
    },
    /// The backend faulted; it keeps the arena in its quarantine.
    Fault(F),
}

impl<K: Ieee80211Capabilities, F: core::fmt::Debug> core::fmt::Debug for PublishError<'_, K, F> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Refused {
                publication,
                reason,
            } => f
                .debug_struct("Refused")
                .field("publication", publication)
                .field("reason", reason)
                .finish(),
            Self::Fault(fault) => f.debug_tuple("Fault").field(fault).finish(),
        }
    }
}

/// How one published arena ended, with the backend's response evidence `R`
/// and lost-air evidence `D`. A response without a transmission is not
/// representable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptEnd<R, D> {
    /// A BlockAck answered the A-MPDU.
    Answered {
        /// The transmission.
        air: Air,
        /// The BlockAck's sequence window.
        report: BlockAckReport,
        /// The backend's evidence of the BlockAck.
        response: R,
    },
    /// An ACK answered the single MPDU.
    Acked {
        /// The transmission.
        air: Air,
        /// The backend's evidence of the ACK.
        response: R,
    },
    /// The PPDU went out and no ACK or BlockAck answered it.
    NoResponse(Air),
    /// The protection exchange drew no CTS; `None` when no frame reached
    /// the air.
    ProtectionFailure(Option<Air>),
    /// Internal contention or a collision ended the attempt. No rate sample.
    Collision(Option<Air>),
    /// Coexistence denied the air. No rate sample.
    AirDenied {
        /// The backend's evidence of the denial.
        denial: D,
        /// The transmissions made before it.
        air: Option<Air>,
    },
    /// The hardware timed out and the backend aborted the publication:
    /// whether the PPDU reached the air is unknown.
    Aborted,
}

impl<R, D> AttemptEnd<R, D> {
    /// The transmission, when the end establishes one.
    pub const fn air(&self) -> Option<Air> {
        match self {
            Self::Answered { air, .. } | Self::Acked { air, .. } | Self::NoResponse(air) => {
                Some(*air)
            }
            Self::ProtectionFailure(air) | Self::Collision(air) | Self::AirDenied { air, .. } => {
                *air
            }
            Self::Aborted => None,
        }
    }

    /// The BlockAck, when one answered the A-MPDU.
    pub const fn block_ack(&self) -> Option<&BlockAckReport> {
        match self {
            Self::Answered { report, .. } => Some(report),
            _ => None,
        }
    }

    /// Whether a frame of the arena may have reached a receiver: a retained
    /// frame is then republished with its Retry bit set.
    pub const fn may_have_reached_air(&self) -> bool {
        matches!(self, Self::Aborted) || self.air().is_some()
    }
}

/// The backend's report of a published arena's end, which
/// [`TxArena::service`] turns into the caller's [`ArenaCompleted`].
#[must_use = "a completion carries its arena's right"]
pub struct Completion<R, D> {
    arena: Arena,
    frames: u8,
    end: AttemptEnd<R, D>,
}

impl<R, D> Completion<R, D> {
    /// The end of the published `arena`, which still holds `frames` frames.
    pub const fn new(arena: Arena, frames: u8, end: AttemptEnd<R, D>) -> Self {
        Self { arena, frames, end }
    }
}

/// A published arena that ended: its frames stay in the arena until
/// [`TxArena::retain`] or [`TxArena::release`] consumes this value, which
/// alone returns the arena's right. Dropping it leaves the arena's frames
/// with the backend for good.
#[must_use = "retain or release the completed arena to settle it"]
#[derive(Debug)]
pub struct ArenaCompleted<R, D> {
    arena: Arena,
    frames: u8,
    end: AttemptEnd<R, D>,
}

impl<R, D> ArenaCompleted<R, D> {
    /// The arena that ended.
    pub const fn id(&self) -> ArenaId {
        self.arena.id()
    }

    /// The number of frames the arena published, positions `0..frames`.
    pub const fn frames(&self) -> u8 {
        self.frames
    }

    /// How it ended.
    pub const fn end(&self) -> &AttemptEnd<R, D> {
        &self.end
    }

    /// The transmission, when the end establishes one.
    pub const fn air(&self) -> Option<Air> {
        self.end.air()
    }
}

/// What the backend does with a completed arena: keep the frames of `keep`,
/// compacted in their relative order to positions `0..keep.count()`, and
/// return every other memory in position order. Only the caller's
/// [`TxArena::retain`] and [`TxArena::release`] create one.
#[derive(Debug, Eq, PartialEq)]
pub struct Settlement {
    arena: ArenaId,
    frames: u8,
    keep: FrameMask,
    retransmission: bool,
}

impl Settlement {
    /// The completed arena.
    pub const fn arena(&self) -> ArenaId {
        self.arena
    }

    /// The frames it published, positions `0..frames`.
    pub const fn frames(&self) -> u8 {
        self.frames
    }

    /// The positions it keeps, all below [`Self::frames`].
    pub const fn keep(&self) -> FrameMask {
        self.keep
    }

    /// Whether the publication may have reached a receiver: the backend
    /// sets the Retry bit of each kept frame it sent (every frame of an
    /// A-MPDU, position 0 of a single MPDU) before it goes again; a kept
    /// frame the publication did not send keeps its header.
    pub const fn retransmission(&self) -> bool {
        self.retransmission
    }
}

/// A refused or failed [`TxArena::retain`].
#[derive(Debug)]
pub enum RetainError<R, D, F> {
    /// The retry mask names a position at or above the completed frames;
    /// the completion comes back unchanged.
    Outside(ArenaCompleted<R, D>),
    /// The backend faulted; it keeps the arena in its quarantine.
    Fault(F),
}

/// The completed arena of the backend `A`.
pub type CompletedOf<'p, 'id, A> =
    ArenaCompleted<<A as TxArena<'p, 'id>>::Response, <A as TxArena<'p, 'id>>::Denial>;

/// The backend `A`'s report of a published arena's end.
pub type CompletionOf<'p, 'id, A> =
    Completion<<A as TxArena<'p, 'id>>::Response, <A as TxArena<'p, 'id>>::Denial>;

/// A refused or failed retention of the backend `A`.
pub type RetainErrorOf<'p, 'id, A> = RetainError<
    <A as TxArena<'p, 'id>>::Response,
    <A as TxArena<'p, 'id>>::Denial,
    <A as TxArena<'p, 'id>>::Fault,
>;

/// The data transmit path of a backend: two arenas of DMA slots, filled
/// one frame at a time and published as one PPDU each.
///
/// Every operation is synchronous. Required methods are the backend's
/// side; the caller uses the provided ones, which hold the arena-right and
/// completion typestate.
pub trait TxArena<'p, 'id: 'p> {
    /// The capabilities the port checked the publication's rate and power
    /// against.
    type Caps: Ieee80211Capabilities;
    /// The memory a frame occupies: the caller's buffer, held by its arena
    /// slot from intake until it is returned.
    type Memory;
    /// What the backend observed since the last service: its sampled
    /// transmit interrupt events, or a deadline wake.
    type Events;
    /// The backend's evidence of an ACK or a BlockAck.
    type Response;
    /// The backend's evidence of a coexistence denial.
    type Denial;
    /// A backend fault: the backend keeps every arena it holds in its
    /// quarantine, and every later operation fails.
    type Fault;

    /// Claim the two idle arenas: `true` exactly once.
    fn claim_arenas(&mut self) -> bool;

    /// The number of slots of each arena, at most [`ARENA_FRAMES`].
    fn capacity(&self) -> u8;

    /// Add a frame at the next position of `arena`: `write` receives the
    /// memory's frame region, writes the MPDU from its first octet and
    /// returns its length without FCS and MIC.
    ///
    /// # Errors
    ///
    /// The memory, as `write` left it, and why the frame was refused.
    fn push(
        &mut self,
        arena: &Arena,
        memory: Self::Memory,
        write: impl FnOnce(&mut [u8]) -> Option<u16>,
    ) -> Result<(), (Self::Memory, IntakeRefused)>;

    /// The number of frames `arena` holds.
    fn len(&self, arena: ArenaId) -> u8;

    /// The arena on the air: published and not yet completed.
    fn on_air(&self) -> Option<ArenaId>;

    /// Whether the backend has an event for the caller to serve, for a
    /// caller that fills an arena and owes the air a quick answer.
    fn event_waiting(&self) -> bool {
        false
    }

    /// Put the arena's frames on the air as one PPDU, positions in order.
    ///
    /// # Errors
    ///
    /// A refusal returns the publication unchanged; a fault quarantines.
    fn publish(
        &mut self,
        publication: ArenaPublication<'p, Self::Caps>,
        now: Instant,
    ) -> Result<(), PublishError<'p, Self::Caps, Self::Fault>>;

    /// Serve `events`: the end of the arena on the air, once it ended.
    ///
    /// # Errors
    ///
    /// A fault quarantines.
    fn complete(
        &mut self,
        events: Self::Events,
        now: Instant,
    ) -> Result<Option<CompletionOf<'p, 'id, Self>>, Self::Fault>;

    /// Keep and return a completed arena's frames as `settlement` says.
    ///
    /// # Errors
    ///
    /// A fault quarantines.
    fn settle(
        &mut self,
        settlement: Settlement,
        returned: impl FnMut(Self::Memory),
    ) -> Result<(), Self::Fault>;

    /// Return every memory of the unpublished `arena`, in position order.
    ///
    /// # Errors
    ///
    /// A fault quarantines.
    fn withdraw(
        &mut self,
        arena: &Arena,
        returned: impl FnMut(Self::Memory),
    ) -> Result<(), Self::Fault>;

    /// When [`Self::service`] must run without an event: the published
    /// arena's hardware deadline.
    fn deadline(&self) -> Option<Instant>;

    /// The rights to the backend's two arenas, once.
    fn arenas(&mut self) -> Option<[Arena; 2]> {
        if self.claim_arenas() {
            Some([Arena(ArenaId::First), Arena(ArenaId::Second)])
        } else {
            None
        }
    }

    /// Add a frame at the next position of `arena`, as [`Self::push`] does,
    /// and name its position.
    ///
    /// # Errors
    ///
    /// The memory and why the frame was refused.
    fn intake(
        &mut self,
        arena: &Arena,
        memory: Self::Memory,
        write: impl FnOnce(&mut [u8]) -> Option<u16>,
    ) -> Result<FrameIndex, (Self::Memory, IntakeRefused)> {
        let len = self.len(arena.id());
        let Some(index) = FrameIndex::checked(len).filter(|_| len < self.capacity()) else {
            return Err((memory, IntakeRefused::Full));
        };
        self.push(arena, memory, write)?;
        Ok(index)
    }

    /// Serve `events`: the completed arena, once the one on the air ended.
    ///
    /// # Errors
    ///
    /// A fault quarantines.
    fn service(
        &mut self,
        events: Self::Events,
        now: Instant,
    ) -> Result<Option<CompletedOf<'p, 'id, Self>>, Self::Fault> {
        Ok(self
            .complete(events, now)?
            .map(|Completion { arena, frames, end }| ArenaCompleted { arena, frames, end }))
    }

    /// Settle `done` keeping the frames of `retry` for another publication,
    /// compacted in their relative order to positions `0..retry.count()`;
    /// every other memory returns through `returned` in position order.
    /// Returns the arena's right.
    ///
    /// # Errors
    ///
    /// `retry` names a position at or above `done.frames()`: `done` comes
    /// back unchanged. A fault quarantines.
    fn retain(
        &mut self,
        done: CompletedOf<'p, 'id, Self>,
        retry: FrameMask,
        returned: impl FnMut(Self::Memory),
    ) -> Result<Arena, RetainErrorOf<'p, 'id, Self>> {
        if !retry.is_within(done.frames) {
            return Err(RetainError::Outside(done));
        }
        let ArenaCompleted { arena, frames, end } = done;
        let settlement = Settlement {
            arena: arena.id(),
            frames,
            keep: retry,
            retransmission: end.may_have_reached_air(),
        };
        self.settle(settlement, returned)
            .map_err(RetainError::Fault)?;
        Ok(arena)
    }

    /// Settle `done` returning every memory through `returned` in position
    /// order. Returns the arena's right.
    ///
    /// # Errors
    ///
    /// A fault quarantines.
    fn release(
        &mut self,
        done: CompletedOf<'p, 'id, Self>,
        returned: impl FnMut(Self::Memory),
    ) -> Result<Arena, Self::Fault> {
        let ArenaCompleted { arena, frames, end } = done;
        let settlement = Settlement {
            arena: arena.id(),
            frames,
            keep: FrameMask::EMPTY,
            retransmission: end.may_have_reached_air(),
        };
        self.settle(settlement, returned)?;
        Ok(arena)
    }
}

#[cfg(test)]
mod tests;
