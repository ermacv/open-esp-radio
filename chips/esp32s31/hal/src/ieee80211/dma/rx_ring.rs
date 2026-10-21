//! Typed ownership and publication for the ESP32-S31 RX DMA ring.
//!
//! This module contains descriptor-walker mechanics only. 802.11 frame
//! decoding remains in the MAC crate, while the semantic MMIO operations are
//! defined by [`crate::ieee80211::dma::rx_dma::RxDma`].

use crate::dma_address;
use crate::settle::Settle;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::ieee80211::dma::ring_index::RingIndex;

use crate::layouts::WifiRxDmaDescriptorControl;

use crate::ieee80211::dma::{
    descriptor::{DESCRIPTOR_BYTES, RxDescriptor, rx_armed_word, rx_detached_word, rx_rearm_word},
    rx_dma::{RxDma, RxDmaBinding},
};

/// Guard value restored by the ROM RX recycler at both DMA-buffer bounds.
///
/// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS]: `esp32s31_rev0_rom.elf`,
/// `rom[]::wDev_AppendRxBlocks` at `0x2f838a7e`, complete size `0x132`, writes
/// `0xdead_beef` at `buffer` and `buffer + descriptor_capacity`.
/// The trailing word lives immediately after the descriptor-advertised
/// capacity, so backing storage must reserve four additional bytes.
pub const RX_BUFFER_SENTINEL: u32 = 0xdead_beef;

/// Immutable descriptor-to-buffer address authority for one DMA lifecycle.
pub type RxDmaBufferAddresses<const COUNT: usize> = [u32; COUNT];

fn rx_buffer_address_snapshot<const COUNT: usize>(
    binding: &RxDmaBufferAddresses<COUNT>,
) -> [u32; COUNT] {
    *binding
}

const RX_DESCRIPTOR_ADDRESS_LOW_MASK: u32 = oer_esp32s31_pac::WifiDescriptorAddressLow::MAX;

/// The descriptors one RX arena publishes: the ring and, right after it, the
/// reserved descriptor R.
///
/// R is the only descriptor `RX_DESCRIPTOR_BASE` ever names. The cold
/// publication of an epoch writes BASE = R with `R.next` = the ring head;
/// every later publication links the accepted tail and rings the reload
/// doorbell. R lies outside the rotation: it is never observed, taken,
/// leased or recycled, and its buffer is not one of the ring's. It is
/// contiguous with the ring so the bound DMA range and the descriptor window
/// cover it.
#[repr(C)]
#[derive(bytemuck::Zeroable)]
pub struct RxDescriptorTable<const COUNT: usize> {
    ring: [RxDescriptor; COUNT],
    reserved: RxDescriptor,
}

impl<const COUNT: usize> RxDescriptorTable<COUNT> {
    pub const fn new() -> Self {
        Self {
            ring: [const { RxDescriptor::new() }; COUNT],
            reserved: RxDescriptor::new(),
        }
    }

    pub const fn ring(&self) -> &[RxDescriptor; COUNT] {
        &self.ring
    }

    pub const fn reserved(&self) -> &RxDescriptor {
        &self.reserved
    }
}

impl<const COUNT: usize> Default for RxDescriptorTable<COUNT> {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether no legitimate receive unit can span the whole ring of `count`
/// descriptors of `buffer` bytes.
///
/// A unit of `count` descriptors carries more than `(count - 1) * buffer`
/// bytes. When that exceeds [`MAX_ENCODED_MPDU_LEN`], the largest MPDU
/// length the vendor encodes, such a unit is no frame: the ring rejects it
/// as `Corrupt` instead of republishing the whole ring, which the reload
/// doorbell cannot do (its link point, the accepted tail, would be part of
/// the group). Each product geometry asserts this at compile time.
///
/// [`MAX_ENCODED_MPDU_LEN`]: crate::ieee80211::dma::descriptor::MAX_ENCODED_MPDU_LEN
pub const fn rx_ring_outspans_any_unit(count: usize, buffer: usize) -> bool {
    match (count.checked_sub(1), buffer) {
        (Some(rest), buffer) => match rest.checked_mul(buffer) {
            Some(bytes) => bytes > crate::ieee80211::dma::descriptor::MAX_ENCODED_MPDU_LEN,
            None => true,
        },
        (None, _) => false,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RxSegment<'a> {
    pub descriptor_address: u32,
    pub descriptor_word0: WifiRxDmaDescriptorControl,
    pub buffer: &'a [u8],
    pub next_descriptor_address: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxRingError {
    Empty,
    Count,
    Address,
    DescriptorOwnerAddress,
    Size,
    Overflow,
    /// An asynchronous upper owner has not returned its detached buffer.
    OutstandingLeases,
    /// Descriptor ownership contradicts the independently retained buffer.
    BufferOwnershipConflict,
    /// The peripheral has not retired its previous reload doorbell.
    ReloadPending,
    /// The receive walker did not acknowledge its enable edge.
    WalkerStartUnconfirmed,
    /// The receive walker did not acknowledge its stop edge.
    WalkerStopUnconfirmed,
    Corrupt,
    ResetRequired,
}

/// One allocation-free observation of an RX descriptor.
///
/// The snapshot copies volatile words once. It is intended for qualification
/// and fault reporting without exposing the descriptor for mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxDescriptorSnapshot {
    pub index: usize,
    pub address: u32,
    pub word0: WifiRxDmaDescriptorControl,
    pub buffer_address: u32,
    pub next_address: u32,
}

/// One ordered, newly completed descriptor interval at the software frontier.
///
/// Descriptors already observed but not yet recycled are skipped, while a
/// not-yet-completed descriptor terminates the snapshot. Consumers must walk
/// `start_index.advance(step)` for `step < descriptor_count`; scanning physical indices
/// from zero reorders packets whenever the live frontier wraps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxCompletedDescriptorFrontier<const COUNT: usize> {
    pub start_index: RingIndex<COUNT>,
    pub descriptor_count: usize,
}

/// Compact proof that a zero-terminated RX list is complete and coherent.
///
/// `valid` means the walk starting at `start_index` visited every descriptor
/// exactly once, followed the expected rotated order, found exactly one
/// terminal node at `tail_index`, retained the configured buffer binding and,
/// for a stopped ring, observed an armed ownership word on every node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxRingTopologySnapshot {
    pub descriptor_base: u32,
    pub start_index: usize,
    pub head_address: u32,
    pub head_next_address: u32,
    pub tail_index: usize,
    pub tail_address: u32,
    pub visited_descriptors: usize,
    pub terminal_descriptors: usize,
    pub valid: bool,
}

/// Raw observations of one epoch's publications and reload settles.
///
/// These counters deliberately record decisions and field observations without
/// assigning hardware meaning to unknown state. They remain attached to the live ring
/// so a later disconnect can explain what preceded a fault. The epoch begins
/// at the ring's preparation, so its one BASE write is counted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxReloadEvidence {
    /// The most `APPEND_DESCRIPTOR_RELOAD` observations one
    /// [`RxRingLive::complete_pending_reload`] made in this epoch, the one
    /// which saw the doorbell clear included. A reload which exhausted its
    /// [`Settle`] counts the observations it made.
    pub max_reload_attempts: u32,
    pub observations: u32,
    pub unknown_upper_with_zero_address: u32,
    /// Settled doorbells rung once more because the walker read NEXT = 0
    /// with LAST short of the new tail: it had exhausted the old list
    /// before it followed the new link. At most one per appended tail.
    pub reload_rerings: u32,
    /// Re-rung doorbells after which NEXT was still zero with LAST short
    /// of the new tail. Each contradicts the doorbell's observed rule and
    /// poisons the arena.
    pub rering_failures: u32,
    pub last_next_low: u32,
    pub last_last_low: Option<u32>,
    /// `RX_DESCRIPTOR_BASE` writes in this epoch: one, the cold
    /// publication of the reserved descriptor.
    pub base_writes: u32,
    /// Frames that landed in the reserved descriptor before the walker
    /// reached the ring: dropped uncopied, the reserved descriptor rearmed.
    pub reserved_landings: u32,
    /// Writes into the reserved descriptor after the walker reached the
    /// ring. Each poisons the arena.
    pub reserved_late_landings: u32,
    /// NEXT reads naming a detached descriptor, whose owner bit is clear:
    /// the walker stopped on that barrier. Impossible while every
    /// publication follows the reserved-descriptor rules; each poisons the
    /// arena.
    pub walker_on_detached: u32,
}

/// The words a static RX arena keeps beside its descriptors: the sticky
/// lifecycle and the reload evidence of its epoch.
///
/// The evidence lives here, not in the movable ring token, so the token
/// (carried through nested async state) stays one pointer wide. Its zero
/// bytes are a reusable arena without evidence: optional values are stored
/// as `value + 1`, so zero means `None`; a 20-bit descriptor address never
/// reaches `u32::MAX`.
#[derive(bytemuck::Zeroable)]
pub struct RxArenaWords {
    lifecycle: AtomicU8,
    max_reload_attempts: AtomicU32,
    observations: AtomicU32,
    unknown_upper_with_zero_address: AtomicU32,
    reload_rerings: AtomicU32,
    rering_failures: AtomicU32,
    last_next_low: AtomicU32,
    last_last_low_plus_one: AtomicU32,
    base_writes: AtomicU32,
    reserved_landings: AtomicU32,
    reserved_late_landings: AtomicU32,
    walker_on_detached: AtomicU32,
    /// Live epochs started on this arena. A frozen cursor names the epoch it
    /// was sampled in, so a cursor of an earlier epoch on the same arena,
    /// whose generation count restarted at zero, never authorizes a take or
    /// recycle in a later one. Not evidence: [`Self::reset_evidence`] keeps it.
    live_epochs: AtomicU32,
}

impl RxArenaWords {
    pub(crate) const fn new() -> Self {
        Self {
            lifecycle: AtomicU8::new(RxDmaArenaState::Reusable as u8),
            max_reload_attempts: AtomicU32::new(0),
            observations: AtomicU32::new(0),
            unknown_upper_with_zero_address: AtomicU32::new(0),
            reload_rerings: AtomicU32::new(0),
            rering_failures: AtomicU32::new(0),
            last_next_low: AtomicU32::new(0),
            last_last_low_plus_one: AtomicU32::new(0),
            base_writes: AtomicU32::new(0),
            reserved_landings: AtomicU32::new(0),
            reserved_late_landings: AtomicU32::new(0),
            walker_on_detached: AtomicU32::new(0),
            live_epochs: AtomicU32::new(0),
        }
    }

    fn live_epoch(&self) -> u32 {
        self.live_epochs.load(Ordering::Relaxed)
    }

    pub(crate) const fn lifecycle(&self) -> &AtomicU8 {
        &self.lifecycle
    }

    fn reset_evidence(&self) {
        for word in [
            &self.max_reload_attempts,
            &self.observations,
            &self.unknown_upper_with_zero_address,
            &self.reload_rerings,
            &self.rering_failures,
            &self.last_next_low,
            &self.last_last_low_plus_one,
            &self.base_writes,
            &self.reserved_landings,
            &self.reserved_late_landings,
            &self.walker_on_detached,
        ] {
            word.store(0, Ordering::Relaxed);
        }
    }

    fn evidence(&self) -> RxReloadEvidence {
        let load = |word: &AtomicU32| word.load(Ordering::Relaxed);
        RxReloadEvidence {
            max_reload_attempts: load(&self.max_reload_attempts),
            observations: load(&self.observations),
            unknown_upper_with_zero_address: load(&self.unknown_upper_with_zero_address),
            reload_rerings: load(&self.reload_rerings),
            rering_failures: load(&self.rering_failures),
            last_next_low: load(&self.last_next_low),
            last_last_low: decode_optional(load(&self.last_last_low_plus_one)),
            base_writes: load(&self.base_writes),
            reserved_landings: load(&self.reserved_landings),
            reserved_late_landings: load(&self.reserved_late_landings),
            walker_on_detached: load(&self.walker_on_detached),
        }
    }
}

fn record_maximum(maximum: &AtomicU32, value: u32) {
    maximum.fetch_max(value, Ordering::Relaxed);
}

fn increment_saturating(counter: &AtomicU32) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}

const fn encode_optional(value: Option<u32>) -> u32 {
    match value {
        Some(value) => value.saturating_add(1),
        None => 0,
    }
}

const fn decode_optional(value: u32) -> Option<u32> {
    value.checked_sub(1)
}

/// Persistent state of the static RX arena, independent of a movable ring
/// capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RxDmaArenaState {
    Reusable = 0,
    Prepared = 1,
    Live = 2,
    ResetRequired = 3,
}

impl RxDmaArenaState {
    pub(crate) fn load(state: &AtomicU8) -> Self {
        match state.load(Ordering::Acquire) {
            0 => Self::Reusable,
            1 => Self::Prepared,
            2 => Self::Live,
            _ => Self::ResetRequired,
        }
    }

    pub(crate) fn store(self, state: &AtomicU8) {
        state.store(self as u8, Ordering::Release);
    }

    pub(crate) fn claim_prepared(state: &AtomicU8) -> bool {
        state
            .compare_exchange(
                Self::Reusable as u8,
                Self::Prepared as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

/// One descriptor whose completion ownership has moved from the MAC to Rust.
///
/// Taking it through [`RxDmaStorage::take_completed`](crate::ieee80211::dma::rx_storage::RxDmaStorage::take_completed) records that this
/// descriptor must not be exposed a second time before its recycle group has
/// been rearmed. The token is deliberately neither `Copy` nor constructible
/// by callers: consuming it at the staging boundary represents the unique
/// software owner of that completed DMA descriptor.
#[derive(Debug, PartialEq, Eq)]
pub struct RxCompletedDescriptor {
    index: usize,
    descriptor_address: u32,
    word0: WifiRxDmaDescriptorControl,
    next_descriptor_address: u32,
}

impl RxCompletedDescriptor {
    pub const fn index(&self) -> usize {
        self.index
    }

    pub const fn descriptor_address(&self) -> u32 {
        self.descriptor_address
    }

    pub const fn word0(&self) -> WifiRxDmaDescriptorControl {
        self.word0
    }

    pub const fn next_descriptor_address(&self) -> u32 {
        self.next_descriptor_address
    }
}

/// Finite snapshot of complete RX units at the live recycle frontier.
///
/// A unit may occupy more than one descriptor. Only the final descriptor has
/// `RX_DONE`; a later terminal descriptor proves that each preceding node in
/// the sequential ring belongs to that same completed unit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxCompletedUnitFrontier {
    pub unit_count: usize,
    pub descriptor_count: usize,
}

/// Completed-descriptor membership for one live ring epoch.
///
/// Three native RV32 words allow the qualified 96-descriptor physical burst
/// reserve without widening every membership operation to software `u64`.
/// Public consumers may
/// count retained descriptors for diagnostics, but mutation remains private
/// to the live ring owner.
const RX_OBSERVED_MASK_CAPACITY: usize = 96;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxObservedMask {
    words: [u32; 3],
}

impl RxObservedMask {
    pub const fn contains(self, index: usize) -> bool {
        index < RX_OBSERVED_MASK_CAPACITY && self.words[index / 32] & (1_u32 << (index % 32)) != 0
    }

    pub const fn count_ones(self) -> u32 {
        self.words[0].count_ones() + self.words[1].count_ones() + self.words[2].count_ones()
    }

    pub const fn is_empty(self) -> bool {
        self.words[0] == 0 && self.words[1] == 0 && self.words[2] == 0
    }

    fn insert(&mut self, index: usize) {
        self.words[index / 32] |= 1_u32 << (index % 32);
    }

    fn insert_group<const COUNT: usize>(&mut self, start: RingIndex<COUNT>, count: usize) {
        for step in 0..count {
            self.insert(start.advance(step).get());
        }
    }

    fn remove_group<const COUNT: usize>(&mut self, start: RingIndex<COUNT>, count: usize) {
        for step in 0..count {
            let index = start.advance(step);
            self.words[index.get() / 32] &= !(1_u32 << (index.get() % 32));
        }
    }

    fn contains_group<const COUNT: usize>(self, start: RingIndex<COUNT>, count: usize) -> bool {
        (0..count).all(|step| self.contains(start.advance(step).get()))
    }
}

/// One ordered hardware cursor bound to a live descriptor generation.
///
/// Addresses can be reused immediately after append. Keeping the generation,
/// live epoch and arena identity private prevents a stale LAST from
/// authorizing mutation of descriptors which have already returned to
/// hardware. Only [`RxRingLive::freeze_cursor`] makes one, so every unit
/// take and frontier is bounded by a LAST the hardware reported for this
/// exact ring generation; a cursor of another arena, live epoch or
/// generation authorizes nothing.
///
/// ```compile_fail,E0451
/// let _forged = oer_esp32s31_hal::ieee80211::dma::rx_ring::RxFrozenCursor {
///     last_descriptor_low: 0,
///     next_descriptor_low: 0,
///     descriptor_base: 0,
///     arena_identity: 0,
///     live_epoch: 0,
///     generation: 0,
/// };
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxFrozenCursor {
    last_descriptor_low: u32,
    next_descriptor_low: u32,
    descriptor_base: u32,
    arena_identity: usize,
    live_epoch: u32,
    generation: u32,
}

impl RxFrozenCursor {
    pub const fn last_descriptor_low(self) -> u32 {
        self.last_descriptor_low
    }

    pub const fn next_descriptor_low(self) -> u32 {
        self.next_descriptor_low
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RxRecycleGroupShape {
    EveryDescriptorTerminal,
    OneCompletedUnit,
    OneCompletedUnitThroughFrozenLast,
}

/// Unique ownership of one complete, possibly chained, RX unit.
///
/// Segment indices are implicit contiguous ring indices beginning at
/// [`head_index`](Self::head_index). Lengths are captured before recycle so a
/// staging owner can copy every DMA segment without rereading mutable
/// descriptor state. A physical ring may be deeper than one maximum-sized
/// receive unit; unit metadata remains bounded separately below.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RxCompletedUnit {
    head_index: usize,
    descriptor_count: usize,
    descriptor_address: u32,
    /// `None` when the total length does not fit the descriptor length field.
    staged_word0: Option<WifiRxDmaDescriptorControl>,
    total_length: usize,
    segment_lengths: RxCompletedUnitLengths,
    /// The frame began in the reserved descriptor and continued into this
    /// unit: this unit is only its tail.
    continues_reserved_landing: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RxCompletedUnitLengths {
    Single(u16),
    Chained([u16; 64]),
}

impl RxCompletedUnit {
    pub const fn head_index(&self) -> usize {
        self.head_index
    }

    pub const fn descriptor_count(&self) -> usize {
        self.descriptor_count
    }

    pub const fn descriptor_address(&self) -> u32 {
        self.descriptor_address
    }

    /// Synthetic single-segment metadata for the independent contiguous
    /// staging copy. It preserves the first descriptor's status bits while
    /// describing the complete unit length and terminal ownership.
    ///
    /// `None` for a chain whose total length exceeds the descriptor's
    /// 14-bit length field: no single descriptor word can
    /// describe it. Such a unit is still a complete owned unit; its consumer
    /// drops it like any other chained unit and recycles its descriptors.
    pub const fn staged_word0(&self) -> Option<WifiRxDmaDescriptorControl> {
        self.staged_word0
    }

    pub const fn total_length(&self) -> usize {
        self.total_length
    }

    /// Whether this unit is the tail of a frame that began in the reserved
    /// descriptor. Its consumer drops it like a chained unit.
    pub const fn continues_reserved_landing(&self) -> bool {
        self.continues_reserved_landing
    }

    pub fn segment_length(&self, step: usize) -> Option<usize> {
        if step >= self.descriptor_count {
            return None;
        }
        match &self.segment_lengths {
            RxCompletedUnitLengths::Single(length) => (step == 0).then_some(usize::from(*length)),
            RxCompletedUnitLengths::Chained(lengths) => Some(usize::from(lengths[step])),
        }
    }
}

/// One live append accepted for publication to the RX descriptor walker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxLiveAppend<const COUNT: usize> {
    pub head_index: RingIndex<COUNT>,
    pub head_address: u32,
    pub tail_index: RingIndex<COUNT>,
    pub descriptor_count: usize,
}

/// Result of one finite observation of the live RX append doorbell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxReloadObservation {
    Pending,
    Settled,
}

/// The reserved descriptor's buffer, as the live ring needs it.
///
/// The storage arena implements it over R's buffer, which is never one of
/// the ring's buffers and is never leased.
pub(crate) trait RxReservedBuffer {
    /// Whether DMA overwrote the buffer's leading recycle guard.
    fn guard_overwritten(&self) -> bool;
    /// Restore both recycle guards. Called only after the walker finished
    /// writing the buffer.
    fn restore_guards(&self) -> Result<(), RxRingError>;
}

/// Validated descriptor memory and private DMA binding, before BASE.
/// Only the MAC HAL can promote this capability after opening common gates.
pub struct RxRingPrepared<'a, const COUNT: usize> {
    ring: RxRingStopped<'a, COUNT>,
}

/// Common MAC gates are open with runtime stop held; BASE is unpublished.
/// Consuming this capability publishes the epoch's sole BASE.
pub struct RxRingCoreReady<'a, const COUNT: usize> {
    ring: RxRingStopped<'a, COUNT>,
}

impl<'a, const COUNT: usize> RxRingPrepared<'a, COUNT> {
    pub fn topology_snapshot(&self) -> RxRingTopologySnapshot {
        self.ring.topology_snapshot()
    }

    pub(crate) fn core_ready(self) -> RxRingCoreReady<'a, COUNT> {
        RxRingCoreReady { ring: self.ring }
    }

    /// Synthetic MAC core edge for DMA/driver models and probe setup only.
    #[cfg(any(test, feature = "test-support", oer_probe))]
    pub fn core_ready_for_test(self) -> RxRingCoreReady<'a, COUNT> {
        self.core_ready()
    }
}

impl<'a, const COUNT: usize> RxRingCoreReady<'a, COUNT> {
    pub fn publish<M: RxDma>(self, mmio: &mut M) -> Result<RxRingStopped<'a, COUNT>, RxRingError> {
        publish_cold_ring_inner(mmio, &self.ring.binding, self.ring.arena)?;
        Ok(self.ring)
    }
}

/// Published zero-terminated RX ring while the hardware walker is stopped.
///
/// This type owns the right to start the descriptor walker. Consuming
/// [`try_start`](Self::try_start) transfers that authority into [`RxRingLive`].
pub struct RxRingStopped<'a, const COUNT: usize> {
    descriptors: &'a [RxDescriptor; COUNT],
    table: &'a RxDescriptorTable<COUNT>,
    reserved_buffer_address: u32,
    descriptor_base: u32,
    buffer_addresses: &'a RxDmaBufferAddresses<COUNT>,
    buffer_size: u16,
    initial_start: RingIndex<COUNT>,
    accepted_tail: RingIndex<COUNT>,
    retained_next_low: u32,
    retained_last_low: u32,
    binding: RxDmaBinding<'a>,
    arena: &'a RxArenaWords,
}

/// Sole software owner of one running S31 RX descriptor frontier.
///
/// The owner tracks three distinct states recovered from
/// `wDev_AppendRxBlocks`: descriptors observed as CPU-owned, the last tail
/// accepted by hardware, and a future tail whose reload doorbell is still in
/// flight. No allocator, global `wDevCtrl`, C ABI or vendor callback is needed.
///
/// Production backing is static. Dropping or forgetting this movable token
/// therefore never releases memory still visible to DMA, but it also does not
/// stop the walker. Normal role owners must consume it through [`Self::try_stop`];
/// an abandoned token poisons the arena until radio reset.
///
/// The token stays on its core: it is not `Sync`.
/// ```compile_fail,E0277
/// fn shared<T: Sync>() {}
/// shared::<oer_esp32s31_hal::ieee80211::dma::rx_ring::RxRingLive<'static, 2>>();
/// ```
pub struct RxRingLive<'a, const COUNT: usize> {
    descriptors: &'a [RxDescriptor; COUNT],
    table: &'a RxDescriptorTable<COUNT>,
    reserved_buffer_address: u32,
    descriptor_base: u32,
    buffer_addresses: &'a RxDmaBufferAddresses<COUNT>,
    buffer_size: u16,
    observed_mask: RxObservedMask,
    recycle_start: RingIndex<COUNT>,
    accepted_tail: RingIndex<COUNT>,
    /// Pending tail encoded as `index + 1`; zero means no reload is in flight.
    /// Ring indices are bounded below `usize::MAX`, so this saves one machine
    /// word over `Option<usize>` without introducing a sentinel address.
    pending_tail_plus_one: usize,
    cursor_generation: u32,
    completion_release_probe_pending: bool,
    /// Whether the first ring take of this epoch proved that the walker
    /// left the reserved descriptor and cleared its link.
    reserved_left: bool,
    /// Whether the pending tail's doorbell was already rung a second time.
    reload_rerung: bool,
    binding: RxDmaBinding<'a>,
    arena: &'a RxArenaWords,
    requires_stop: bool,
}

/// RxDescriptor storage authority after the hardware walker is confirmed off.
///
/// Live frontier bookkeeping is deliberately discarded at this lifecycle
/// edge. A later association must rebuild and republish the ring through
/// [`RxDmaStorage::prepare_halted`](crate::ieee80211::dma::rx_storage::RxDmaStorage::prepare_halted), never resume descriptors from the previous
/// peer epoch.
pub struct RxRingHalted<'a, const COUNT: usize> {
    table: &'a RxDescriptorTable<COUNT>,
    descriptors: &'a [RxDescriptor; COUNT],
    reserved_buffer_address: u32,
    descriptor_base: u32,
    buffer_addresses: &'a RxDmaBufferAddresses<COUNT>,
    buffer_size: u16,
    arena: &'a RxArenaWords,
}

impl<'a, const COUNT: usize> RxRingHalted<'a, COUNT> {
    /// Whether this halted ring was built from the storage owning `arena`.
    pub(crate) fn belongs_to_arena(&self, arena: &RxArenaWords) -> bool {
        core::ptr::eq(self.arena, arena)
    }

    pub const fn descriptor_base(&self) -> u32 {
        self.descriptor_base
    }

    pub const fn descriptors(&self) -> &'a [RxDescriptor; COUNT] {
        self.descriptors
    }

    pub const fn buffer_addresses(&self) -> &RxDmaBufferAddresses<COUNT> {
        self.buffer_addresses
    }

    /// Observe one halted descriptor without recovering mutation authority.
    ///
    /// A halted ring can contain the terminal image of the preceding DMA
    /// epoch. This snapshot is useful for fault evidence before the next
    /// [`RxDmaStorage::prepare_halted`](crate::ieee80211::dma::rx_storage::RxDmaStorage::prepare_halted) rebuilds every descriptor.
    pub fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        descriptor_snapshot(self.descriptors, self.descriptor_base, index)
    }

    /// Rebuild the stopped ring for a new ownership epoch.
    ///
    /// Failure returns the halted authority even if hardware observations or
    /// descriptor preparation rejected the attempt. The caller can then
    /// retry after a higher-level reset without stealing the static storage.
    pub(crate) fn prepare_owned<M, F, R>(
        self,
        mmio: &mut M,
        buffer_size: u32,
        prepare_buffer: F,
        prepare_reserved_buffer: R,
    ) -> Result<RxRingStopped<'a, COUNT>, (Self, RxRingError)>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
        R: FnOnce() -> Result<(), RxRingError>,
    {
        if u32::from(self.buffer_size) != buffer_size {
            return Err((self, RxRingError::Size));
        }
        let lifecycle = self.arena.lifecycle();
        let buffer_addresses = self.buffer_addresses;
        match RxRingStopped::prepare_inner(
            mmio,
            self.table,
            self.descriptor_base,
            buffer_addresses,
            self.reserved_buffer_address,
            buffer_size,
            prepare_buffer,
            prepare_reserved_buffer,
            self.arena,
        ) {
            // A halted ring retains the common-core proof established before
            // its first BASE. Warm rebuilds keep that core open and RX stopped.
            Ok(prepared) => prepared
                .core_ready()
                .publish(mmio)
                .map_err(|error| (self, error)),
            Err(error) => {
                if mmio.walker_enabled() {
                    // Preparation failed without proving that the walker is
                    // stopped. The returned token preserves arena ownership
                    // for reset diagnostics, but must never authorize another
                    // descriptor rebuild in this radio epoch.
                    RxDmaArenaState::ResetRequired.store(lifecycle);
                }
                Err((self, error))
            }
        }
    }
}

impl<'a, const COUNT: usize> RxRingStopped<'a, COUNT> {
    /// Discard a prepared-but-never-started epoch while retaining descriptor
    /// storage authority.
    ///
    /// `RxRingStopped` can exist only after the walker was observed disabled.
    /// No descriptor has crossed back to hardware until `try_start` succeeds,
    /// so a higher-level scan failure may safely return to the peer-neutral
    /// halted frontier without manufacturing or re-reading static addresses.
    pub fn into_halted(self) -> RxRingHalted<'a, COUNT> {
        RxRingHalted {
            table: self.table,
            descriptors: self.descriptors,
            reserved_buffer_address: self.reserved_buffer_address,
            descriptor_base: self.descriptor_base,
            buffer_addresses: self.buffer_addresses,
            buffer_size: self.buffer_size,
            arena: self.arena,
        }
    }

    /// Stops the walker and prepares all buffers for one physical-order
    /// cold list beginning at the retained hardware cursor, or descriptor zero
    /// when the preceding list was exhausted, behind the reserved descriptor.
    ///
    /// `prepare_buffer` must restore any buffer-side DMA contract for `index`;
    /// for the S31 ROM layout this means the two `0xdead_beef` sentinels, and
    /// `prepare_reserved_buffer` does the same for the reserved descriptor's
    /// buffer at `reserved_buffer_address`. Both are invoked only while the
    /// walker is confirmed stopped.
    ///
    /// The later core-ready publication is the epoch's only `RX_DESCRIPTOR_BASE` write:
    /// BASE = R, `R.next` = the ring head, R armed with its own buffer. A
    /// frame the walker writes into R is dropped uncopied; the first ring
    /// take clears `R.next` ([`RxRingLive`]).
    ///
    /// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS,ROM_REV0_HAL_MAC_RX_GATE,
    /// ROM_REV0_HAL_MAC_RX_LAST_DESCRIPTOR]. A retained nonzero NEXT cursor
    /// is authoritative because WALKER_ENABLE does not clear it; the rebuilt
    /// zero-terminated list is rotated to that exact descriptor. A retained
    /// NEXT naming R resumes at R, which is this cold publication itself.
    ///
    /// Only the storage arena calls this, through
    /// [`RxDmaStorage::prepare_ring`](crate::ieee80211::dma::rx_storage::RxDmaStorage::prepare_ring)
    /// and [`RxDmaStorage::prepare_halted`](crate::ieee80211::dma::rx_storage::RxDmaStorage::prepare_halted),
    /// which bind every descriptor to its arena buffer.
    #[allow(
        clippy::too_many_arguments,
        reason = "each argument is one validated part of the arena's layout"
    )]
    pub(crate) fn prepare_inner<M, F, R>(
        mmio: &mut M,
        table: &'a RxDescriptorTable<COUNT>,
        descriptor_base: u32,
        buffer_addresses: &'a RxDmaBufferAddresses<COUNT>,
        reserved_buffer_address: u32,
        buffer_size: u32,
        mut prepare_buffer: F,
        prepare_reserved_buffer: R,
        arena: &'a RxArenaWords,
    ) -> Result<RxRingPrepared<'a, COUNT>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
        R: FnOnce() -> Result<(), RxRingError>,
    {
        validate_live_ring_geometry::<COUNT>()?;
        let buffer_size_u16 = u16::try_from(buffer_size).map_err(|_| RxRingError::Size)?;
        let Some(armed_word) = rx_armed_word(buffer_size) else {
            return Err(RxRingError::Size);
        };
        let descriptors = table.ring();
        let binding = RxDmaBinding::new(table, descriptor_base, arena).ok_or(RxRingError::Count)?;
        if mmio.walker_enabled() {
            disable_receive_inner(mmio, &binding, arena)?;
        }
        // The epoch's evidence starts here, so its one BASE write counts.
        arena.reset_evidence();
        // NEXT and LAST belong to the walker. They are stable handoff
        // frontiers only after WALKER_ENABLE has been confirmed clear.
        mmio.fence();
        let retained_next_low = mmio.next_descriptor_low();
        let retained_last_low = mmio.last_descriptor_low();
        // Disabling WALKER_ENABLE does not reset RX_NEXT_DESCRIPTOR. A later
        // enable resumes that cursor when it is nonzero, even if software has
        // written a different BASE. Rebuild the zero-terminated list from the
        // retained cursor so hardware and the Rust recycle frontier name the
        // same first descriptor. An exhausted cursor is zero, and a cursor
        // naming R resumes at the cold publication itself; in both cases the
        // list starts at physical slot zero behind R.
        let reserved_low = binding.reserved_address() & RX_DESCRIPTOR_ADDRESS_LOW_MASK;
        let initial_start = if retained_next_low == 0 || retained_next_low == reserved_low {
            RingIndex::<COUNT>::checked(0).ok_or(RxRingError::Count)?
        } else {
            // A nonzero cursor is what hardware resumes from. Falling back to
            // BASE would publish a topology which Rust considers valid while
            // the walker follows an unrelated address, so fail closed.
            descriptor_index::<COUNT>(retained_next_low, descriptor_base)
                .ok_or(RxRingError::Corrupt)?
        };

        let buffer_addresses_snapshot = rx_buffer_address_snapshot(buffer_addresses);
        for index in 0..COUNT {
            prepare_buffer(index)?;
        }
        prepare_reserved_buffer()?;
        build_cold_ring_inner(
            descriptors,
            descriptor_base,
            &buffer_addresses_snapshot,
            buffer_size,
        )?;
        relink_rotated_ring(
            descriptors,
            descriptor_base,
            &buffer_addresses_snapshot,
            initial_start,
        )?;
        let head = descriptor_address(descriptor_base, initial_start.get())?;
        // SAFETY: `prepare_inner`'s only callers, `PlacedRxDma::prepare_ring`
        // and `prepare_halted`, pass the DMA addresses of their own placed
        // descriptor table and buffers: the link keeps them in the DMA window
        // and the storage stays allocated for the DMA epoch.
        #[allow(unsafe_code, reason = "typed addresses of the placed arena")]
        let (reserved_buffer, head) = unsafe {
            (
                dma_address::placed(reserved_buffer_address),
                dma_address::placed_link(head),
            )
        };
        table
            .reserved()
            .publish_owned(armed_word, reserved_buffer, head);

        Ok(RxRingPrepared {
            ring: Self {
                descriptors,
                table,
                reserved_buffer_address,
                descriptor_base,
                buffer_addresses,
                buffer_size: buffer_size_u16,
                initial_start,
                accepted_tail: initial_start.previous(),
                retained_next_low,
                retained_last_low,
                binding,
                arena,
            },
        })
    }

    pub const fn initial_start(&self) -> RingIndex<COUNT> {
        self.initial_start
    }

    pub const fn accepted_tail(&self) -> RingIndex<COUNT> {
        self.accepted_tail
    }

    pub const fn retained_last_low(&self) -> u32 {
        self.retained_last_low
    }

    pub const fn retained_next_low(&self) -> u32 {
        self.retained_next_low
    }

    pub const fn buffer_addresses(&self) -> &RxDmaBufferAddresses<COUNT> {
        self.buffer_addresses
    }

    /// Observe one descriptor without exposing mutation authority.
    pub fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        descriptor_snapshot(self.descriptors, self.descriptor_base, index)
    }

    /// Validate the complete stopped list before it becomes hardware-owned.
    pub fn topology_snapshot(&self) -> RxRingTopologySnapshot {
        let buffer_addresses = rx_buffer_address_snapshot(self.buffer_addresses());
        ring_topology_snapshot(
            self.descriptors,
            self.descriptor_base,
            &buffer_addresses,
            self.initial_start,
            true,
        )
    }

    /// The reserved descriptor R, published ahead of the ring.
    pub fn reserved_snapshot(&self) -> RxDescriptorSnapshot {
        reserved_snapshot(self.table, self.descriptor_base)
    }

    /// Whether R is armed with its own buffer and links to the ring head.
    fn reserved_published(&self) -> bool {
        let reserved = self.table.reserved();
        descriptor_address(self.descriptor_base, self.initial_start.get())
            .is_ok_and(|head| reserved.next_address() == head)
            && reserved.buffer_address() == self.reserved_buffer_address
            && rx_armed_word(u32::from(self.buffer_size)) == Some(reserved.word0())
    }

    /// Opens the walker and consumes the stopped-state authority.
    ///
    /// The caller owns any platform-specific settle delay between
    /// ring preparation and this edge.
    pub fn try_start<M: RxDma>(
        self,
        mmio: &mut M,
    ) -> Result<RxRingLive<'a, COUNT>, (Self, RxRingError)> {
        if RxDmaArenaState::load(self.arena.lifecycle()) == RxDmaArenaState::ResetRequired {
            return Err((self, RxRingError::ResetRequired));
        }
        if !self.topology_snapshot().valid || !self.reserved_published() {
            return Err((self, RxRingError::Corrupt));
        }
        if let Err(error) = enable_receive_inner(mmio, &self.binding) {
            if mmio.walker_enabled() {
                // A failed start which nevertheless observes WALKER_ENABLE
                // cannot return an honestly stopped capability. Keep the
                // static backing quarantined; the returned value exists only
                // to preserve diagnostics/ownership until radio reset.
                RxDmaArenaState::ResetRequired.store(self.arena.lifecycle());
            }
            return Err((self, error));
        }
        let Some(live_epoch) = self.arena.live_epoch().checked_add(1) else {
            // A frozen cursor names its epoch; a wrapped count could let a
            // cursor of an earlier epoch authorize this one.
            RxDmaArenaState::ResetRequired.store(self.arena.lifecycle());
            return Err((self, RxRingError::Corrupt));
        };
        self.arena.live_epochs.store(live_epoch, Ordering::Relaxed);
        RxDmaArenaState::Live.store(self.arena.lifecycle());
        Ok(RxRingLive {
            descriptors: self.descriptors,
            table: self.table,
            reserved_buffer_address: self.reserved_buffer_address,
            descriptor_base: self.descriptor_base,
            buffer_addresses: self.buffer_addresses,
            buffer_size: self.buffer_size,
            observed_mask: RxObservedMask::default(),
            recycle_start: self.initial_start,
            accepted_tail: self.accepted_tail,
            pending_tail_plus_one: 0,
            cursor_generation: 0,
            completion_release_probe_pending: false,
            reserved_left: false,
            reload_rerung: false,
            binding: self.binding,
            arena: self.arena,
            requires_stop: true,
        })
    }
}

impl<'a, const COUNT: usize> RxRingLive<'a, COUNT> {
    pub const fn descriptor_base(&self) -> u32 {
        self.descriptor_base
    }

    pub(crate) const fn descriptors(&self) -> &'a [RxDescriptor; COUNT] {
        self.descriptors
    }

    fn bound_buffer_address(&self, index: usize) -> Option<u32> {
        self.buffer_addresses.get(index).copied()
    }

    pub const fn buffer_addresses(&self) -> &RxDmaBufferAddresses<COUNT> {
        self.buffer_addresses
    }

    /// Return whether this live owner was constructed by the storage arena
    /// carrying `lifecycle`.
    ///
    /// `RxDmaStorage::prepare_ring` validates the complete immutable address
    /// table before it installs this exact arena-local lifecycle capability.
    /// The capability then travels through stopped/live/halted typestates, so
    /// pointer identity is a constant-time proof of the same binding during a
    /// live epoch. A ring of another arena fails this identity check.
    pub(crate) fn belongs_to_arena(&self, arena: &RxArenaWords) -> bool {
        core::ptr::eq(self.arena, arena)
    }

    /// Observe one live descriptor without transferring its ownership.
    pub fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        descriptor_snapshot(self.descriptors, self.descriptor_base, index)
    }

    /// Walk the links from the current recycle frontier.
    ///
    /// Unlike the stopped proof this does not validate ownership words: the
    /// hardware may be changing those words while the snapshot is taken.
    /// `valid` still proves the address, link, buffer and terminal topology
    /// observed during this finite walk.
    pub fn topology_snapshot(&self) -> RxRingTopologySnapshot {
        let buffer_addresses = rx_buffer_address_snapshot(self.buffer_addresses());
        ring_topology_snapshot(
            self.descriptors,
            self.descriptor_base,
            &buffer_addresses,
            self.recycle_start,
            false,
        )
    }

    /// The reload evidence of this epoch.
    pub fn reload_evidence(&self) -> RxReloadEvidence {
        self.arena.evidence()
    }

    /// The reserved descriptor R, published ahead of the ring.
    pub fn reserved_snapshot(&self) -> RxDescriptorSnapshot {
        reserved_snapshot(self.table, self.descriptor_base)
    }

    /// Whether the first ring take of this epoch cleared R's link.
    pub const fn reserved_left(&self) -> bool {
        self.reserved_left
    }

    fn record_reload_observation(
        &mut self,
        next_descriptor: crate::ieee80211::dma::rx_dma::RxDmaNextDescriptor,
        last_descriptor_low: Option<u32>,
    ) {
        let arena = self.arena;
        increment_saturating(&arena.observations);
        if next_descriptor.address_low() == 0 && next_descriptor.has_unknown_upper_state() {
            increment_saturating(&arena.unknown_upper_with_zero_address);
        }
        arena
            .last_next_low
            .store(next_descriptor.address_low(), Ordering::Relaxed);
        arena
            .last_last_low_plus_one
            .store(encode_optional(last_descriptor_low), Ordering::Relaxed);
    }

    /// Check the reserved descriptor R once per service pass.
    ///
    /// Before the first ring take a frame may land in R: the cold
    /// publication points the walker at it. Such a landing is counted and
    /// dropped uncopied, and R's ownership word and buffer guards are
    /// rearmed. That rewrite is safe: `RX_DONE` on R means the DMA finished
    /// with R's buffer, and R's link, which the walker follows next, is not
    /// touched. A frame still being written into R (guard overwritten, no
    /// `RX_DONE`) is left alone; the first take accounts for it.
    ///
    /// After the first take nothing names R: any write into it (`RX_DONE`,
    /// an overwritten guard, LAST naming R) is a late landing. It is counted
    /// and fails closed: the arena needs a radio reset.
    pub(crate) fn observe_reserved_owned<M: RxDma, B: RxReservedBuffer>(
        &mut self,
        mmio: &mut M,
        buffer: &B,
    ) -> Result<(), RxRingError> {
        let reserved = self.table.reserved();
        let word0 = reserved.word0();
        if !self.reserved_left {
            if word0.rx_done() {
                increment_saturating(&self.arena.reserved_landings);
                if let Err(error) = self.rearm_reserved(word0, buffer) {
                    self.require_reset();
                    return Err(error);
                }
            }
            return Ok(());
        }
        let reserved_low = self.binding.reserved_address() & RX_DESCRIPTOR_ADDRESS_LOW_MASK;
        if word0.rx_done()
            || buffer.guard_overwritten()
            || mmio.last_descriptor_low() == reserved_low
        {
            increment_saturating(&self.arena.reserved_late_landings);
            self.require_reset();
            return Err(RxRingError::Corrupt);
        }
        Ok(())
    }

    /// Rearm R's buffer guards and, after a terminal landing, its ownership
    /// word. R's link is not touched.
    fn rearm_reserved<B: RxReservedBuffer>(
        &self,
        word0: WifiRxDmaDescriptorControl,
        buffer: &B,
    ) -> Result<(), RxRingError> {
        buffer.restore_guards()?;
        if word0.rx_done() {
            self.table
                .reserved()
                .publish_word0(rx_rearm_word(word0).ok_or(RxRingError::Size)?);
        }
        Ok(())
    }

    /// The first ring take of the epoch: account for a frame in R and clear
    /// R's link. Returns whether the frame in R continued into the ring,
    /// so the taken unit is only its tail.
    ///
    /// Proof that the walker left R: a ring descriptor completed, and the
    /// walker reaches the ring only through `R.next` (the cold publication)
    /// or through a NEXT it resumed with (a rotated restart, which never
    /// visits R). Either way it has already fetched `R.next`, and nothing
    /// will name R again: BASE is written once per epoch and no ring link
    /// names R. Clearing `R.next` makes R a dead end, so no walk can
    /// re-enter the ring through a stale head. The leading guard is read
    /// after the ring descriptor's `RX_DONE`, so R's bytes, which precede
    /// it on the walk, are visible.
    fn leave_reserved<B: RxReservedBuffer>(&mut self, buffer: &B) -> Result<bool, RxRingError> {
        let reserved = self.table.reserved();
        let word0 = reserved.word0();
        let landed = word0.rx_done();
        let continued = !landed && buffer.guard_overwritten();
        if landed || continued {
            increment_saturating(&self.arena.reserved_landings);
            self.rearm_reserved(word0, buffer)?;
        }
        reserved.publish_next(None);
        self.reserved_left = true;
        Ok(continued)
    }

    /// The word a completed, observed descriptor `index` carries once its
    /// buffer is detached: its completed image with the owner bit clear
    /// ([`rx_detached_word`]). Validated before the buffer leaves the ring,
    /// so [`Self::publish_detached`] cannot fail after it did.
    pub(crate) fn detached_word(
        &self,
        index: usize,
    ) -> Result<WifiRxDmaDescriptorControl, RxRingError> {
        let descriptor = self.descriptors.get(index).ok_or(RxRingError::Count)?;
        if !self.observed_mask.contains(index) {
            return Err(RxRingError::Corrupt);
        }
        let word0 = descriptor.word0();
        if !word0.rx_done() {
            return Err(RxRingError::Corrupt);
        }
        Ok(rx_detached_word(word0))
    }

    /// Clear the owner bit of descriptor `index`, whose buffer leaves the
    /// ring into a lease, and fence, before the lease is handed out.
    ///
    /// The second layer next to R: a walker that ever reached this
    /// descriptor stops on it without writing the leased buffer
    /// (SOURCE\[HIL_W1_OWNER_BIT_2026_10_18]). The descriptor is observed,
    /// so the walker is done with it; its link and buffer stay. This holds
    /// for the accepted tail too: a doorbell with NEXT = 0 follows
    /// `LAST.next` without checking LAST's owner bit (H1). The rearm sets
    /// the bit again ([`rx_rearm_word`]).
    pub(crate) fn publish_detached<M: RxDma>(
        &mut self,
        mmio: &mut M,
        index: usize,
        word0: WifiRxDmaDescriptorControl,
    ) {
        if let Some(descriptor) = self.descriptors.get(index) {
            descriptor.publish_word0(word0);
        }
        mmio.fence();
    }

    /// Whether the NEXT low address `next_low` names a detached ring
    /// descriptor: observed, with its owner bit clear.
    fn names_detached(&self, next_low: u32) -> bool {
        next_low != 0
            && descriptor_index::<COUNT>(next_low, self.descriptor_base).is_some_and(|index| {
                self.observed_mask.contains(index.get())
                    && self
                        .descriptors
                        .get(index.get())
                        .is_some_and(|descriptor| !descriptor.word0().owner())
            })
    }

    /// Fail closed when NEXT names a detached descriptor: the walker stopped
    /// on the owner-bit barrier, which the reserved descriptor's rules make
    /// impossible. Counted in [`RxReloadEvidence::walker_on_detached`].
    fn check_walker_on_detached(&mut self, next_low: u32) -> Result<(), RxRingError> {
        if !self.names_detached(next_low) {
            return Ok(());
        }
        increment_saturating(&self.arena.walker_on_detached);
        self.require_reset();
        Err(RxRingError::Corrupt)
    }

    /// Irreversibly poison the static arena after an ownership invariant can
    /// no longer be proven. The live token may still be retained for reset
    /// diagnostics, but no later stop observation can make this arena reusable.
    pub fn require_reset(&mut self) {
        RxDmaArenaState::ResetRequired.store(self.arena.lifecycle());
    }

    /// Stop the DMA walker and consume the live frontier authority.
    ///
    /// A failed hardware confirmation returns the complete live owner. This
    /// prevents lifecycle code from rebuilding descriptors while DMA may
    /// still own them.
    #[allow(
        clippy::result_large_err,
        reason = "failure must return the exact allocation-free live DMA owner"
    )]
    pub fn try_stop<M: RxDma>(
        mut self,
        mmio: &mut M,
    ) -> Result<RxRingHalted<'a, COUNT>, (Self, RxRingError)> {
        if let Err(error) = disable_receive_inner(mmio, &self.binding, self.arena) {
            return Err((self, error));
        }
        let halted = RxRingHalted {
            table: self.table,
            descriptors: self.descriptors,
            reserved_buffer_address: self.reserved_buffer_address,
            descriptor_base: self.descriptor_base,
            buffer_addresses: self.buffer_addresses,
            buffer_size: self.buffer_size,
            arena: self.arena,
        };
        let lifecycle = self.arena.lifecycle();
        if RxDmaArenaState::load(lifecycle) != RxDmaArenaState::ResetRequired {
            // The halted token still owns the arena. Only that exact token may
            // rebuild the next epoch; a new root prepare must remain blocked.
            RxDmaArenaState::Prepared.store(lifecycle);
        }
        self.requires_stop = false;
        Ok(halted)
    }

    /// Snapshot the contiguous, newly completed interval after the observed
    /// prefix at the current recycle frontier.
    ///
    /// The returned count is a finite receive epoch. A caller may subsequently
    /// take and recycle exactly this many descriptors without accidentally
    /// extending the same service pass to descriptors completed after the
    /// snapshot. This is important when recycled descriptors can already be
    /// filled again while the task is still draining an RX-success wake.
    pub fn completed_descriptor_frontier(&self) -> RxCompletedDescriptorFrontier<COUNT> {
        let observed = self.observed_prefix_len();
        if observed == COUNT {
            return RxCompletedDescriptorFrontier {
                start_index: self.recycle_start,
                descriptor_count: 0,
            };
        }

        let start_index = self.recycle_start.advance(observed);
        let mut completed = 0;
        while observed + completed < COUNT {
            let index = start_index.advance(completed);
            if self.observed_mask.contains(index.get())
                || !self.descriptors[index.get()].word0().rx_done()
            {
                break;
            }
            completed += 1;
        }
        RxCompletedDescriptorFrontier {
            start_index,
            descriptor_count: completed,
        }
    }

    /// Number of newly completed descriptors beginning exactly at the recycle
    /// frontier. This legacy snapshot deliberately stops at an already
    /// observed descriptor; finite staged consumers should use
    /// [`Self::completed_descriptor_frontier`] to continue after that prefix.
    pub fn completed_frontier_len(&self) -> usize {
        let mut completed = 0;
        while completed < COUNT {
            let index = self.recycle_start.advance(completed);
            if self.observed_mask.contains(index.get())
                || !self.descriptors[index.get()].word0().rx_done()
            {
                break;
            }
            completed += 1;
        }
        completed
    }

    /// The descriptors after the observed prefix which one frozen cursor
    /// releases to software, through its LAST inclusive, and whether the
    /// cursor proves the finite hardware list exhausted.
    ///
    /// SOURCE: the complete vendor walker
    /// `libpp[wdev.o]::wdevProcessRxSucDataAll` snapshots `libpp[hal_mac_rx.o]::hal_mac_rx_get_last_dscr` before touching the software list,
    /// processes through that descriptor, then refreshes the snapshot before
    /// extending the same pass. `RX_DONE` alone is therefore not sufficient
    /// ownership evidence: its store may be observed before the MAC has
    /// finished advancing its descriptor frontier. Returning such a node to
    /// the live append list can overwrite `next` while hardware still needs
    /// it, terminating RX after the first frame.
    ///
    /// A zero NEXT with LAST at the accepted tail proves that the finite
    /// hardware list was exhausted. In that state every link from the
    /// software head through LAST has been consumed, even when an unused
    /// intermediate buffer retained its recycle guard. The complete vendor
    /// `libpp[wdev.o]::wdevProcessRxSucDataAll` likewise walks every node through its saved
    /// LAST and uses only the terminal `RX_DONE` (bit 30) to delimit receive
    /// units.
    ///
    /// `None` for a cursor of another ring, live epoch or descriptor
    /// generation, and for a LAST outside the accepted list after the
    /// observed prefix.
    fn frozen_cursor_descriptor_limit(&self, cursor: RxFrozenCursor) -> Option<(usize, bool)> {
        if !self.frozen_cursor_current(cursor) {
            return None;
        }
        let last_index =
            descriptor_index::<COUNT>(cursor.last_descriptor_low, self.descriptor_base)?;
        let observed = self.observed_prefix_len();
        if observed == COUNT {
            return None;
        }
        let last_distance = self.recycle_start.distance_to(last_index);
        let accepted_distance = self.recycle_start.distance_to(self.accepted_tail);
        if last_distance < observed || last_distance > accepted_distance {
            return None;
        }
        let exhausted = cursor.next_descriptor_low == 0 && last_index == self.accepted_tail;
        Some((last_distance + 1 - observed, exhausted))
    }

    /// Whether one frozen cursor releases descriptor `index` to software: it
    /// lies from the recycle frontier through the cursor's LAST inclusive.
    /// `false` for a cursor of another ring, live epoch or descriptor
    /// generation, for an index outside the ring and for a LAST outside the
    /// accepted list.
    pub(crate) fn frozen_cursor_releases_index(
        &self,
        cursor: RxFrozenCursor,
        index: usize,
    ) -> bool {
        if !self.frozen_cursor_current(cursor) {
            return false;
        }
        let (Some(index), Some(last_index)) = (
            RingIndex::<COUNT>::checked(index),
            descriptor_index::<COUNT>(cursor.last_descriptor_low, self.descriptor_base),
        ) else {
            return false;
        };
        let last_distance = self.recycle_start.distance_to(last_index);
        last_distance <= self.recycle_start.distance_to(self.accepted_tail)
            && self.recycle_start.distance_to(index) <= last_distance
    }

    /// Snapshot complete RX units, including units split across descriptors,
    /// through the LAST of one frozen cursor
    /// ([`Self::frozen_cursor_descriptor_limit`]).
    ///
    /// A non-terminal descriptor is never reported by itself: the scan only
    /// extends `descriptor_count` after observing a later `RX_DONE` terminal,
    /// and `nonterminal_consumed` supplies buffer-side evidence for each
    /// non-terminal descriptor. An armed descriptor and a consumed full
    /// non-terminal descriptor can have the same ownership word; the storage
    /// owner disambiguates them by the buffer guard restored at recycle.
    pub(crate) fn completed_unit_frontier_through_cursor_with<F>(
        &self,
        cursor: RxFrozenCursor,
        nonterminal_consumed: F,
    ) -> RxCompletedUnitFrontier
    where
        F: FnMut(usize) -> bool,
    {
        match self.frozen_cursor_descriptor_limit(cursor) {
            None => RxCompletedUnitFrontier::default(),
            Some((limit, true)) => self.completed_unit_frontier_with_limit(limit, |_| true),
            Some((limit, false)) => {
                self.completed_unit_frontier_with_limit(limit, nonterminal_consumed)
            }
        }
    }

    /// Snapshot only the first complete unit through the LAST of one frozen
    /// cursor. Unlike [`Self::completed_unit_frontier_through_cursor_with`],
    /// a run of single-descriptor frames is deliberately truncated to one
    /// unit so callers can prove and recycle each link before taking the
    /// next owner.
    pub(crate) fn first_completed_unit_frontier_through_cursor_with<F>(
        &self,
        cursor: RxFrozenCursor,
        nonterminal_consumed: F,
    ) -> RxCompletedUnitFrontier
    where
        F: FnMut(usize) -> bool,
    {
        match self.frozen_cursor_descriptor_limit(cursor) {
            None => RxCompletedUnitFrontier::default(),
            Some((limit, true)) => self.first_completed_unit_frontier_with_limit(limit, |_| true),
            Some((limit, false)) => {
                self.first_completed_unit_frontier_with_limit(limit, nonterminal_consumed)
            }
        }
    }

    fn completed_unit_frontier_with_limit<F>(
        &self,
        descriptor_limit: usize,
        mut nonterminal_consumed: F,
    ) -> RxCompletedUnitFrontier
    where
        F: FnMut(usize) -> bool,
    {
        if COUNT == 0 || COUNT > RX_OBSERVED_MASK_CAPACITY {
            return RxCompletedUnitFrontier::default();
        }
        let observed = self.observed_prefix_len();
        let descriptor_limit = descriptor_limit.min(COUNT.saturating_sub(observed));
        if descriptor_limit == 0 {
            return RxCompletedUnitFrontier::default();
        }
        let first_index = self.recycle_start.advance(observed);
        if self.descriptors[first_index.get()].word0().rx_done() {
            let mut completed = 0;
            while completed < descriptor_limit {
                let index = first_index.advance(completed);
                if self.observed_mask.contains(index.get())
                    || !self.descriptors[index.get()].word0().rx_done()
                {
                    break;
                }
                completed += 1;
            }
            return RxCompletedUnitFrontier {
                unit_count: completed,
                descriptor_count: completed,
            };
        }
        if !nonterminal_consumed(first_index.get()) {
            return RxCompletedUnitFrontier::default();
        }
        // Only a later terminal can distinguish a consumed non-terminal
        // segment from an ordinary armed descriptor. Report at most this one
        // chain; a following unit belongs to the next finite service epoch.
        for step in 1..descriptor_limit.min(64) {
            let index = first_index.advance(step);
            if self.observed_mask.contains(index.get()) {
                break;
            }
            if self.descriptors[index.get()].word0().rx_done() {
                return RxCompletedUnitFrontier {
                    unit_count: 1,
                    descriptor_count: step + 1,
                };
            }
            if !nonterminal_consumed(index.get()) {
                break;
            }
        }
        RxCompletedUnitFrontier::default()
    }

    fn first_completed_unit_frontier_with_limit<F>(
        &self,
        descriptor_limit: usize,
        mut nonterminal_consumed: F,
    ) -> RxCompletedUnitFrontier
    where
        F: FnMut(usize) -> bool,
    {
        if COUNT == 0 || COUNT > RX_OBSERVED_MASK_CAPACITY {
            return RxCompletedUnitFrontier::default();
        }
        let observed = self.observed_prefix_len();
        let descriptor_limit = descriptor_limit.min(COUNT.saturating_sub(observed));
        if descriptor_limit == 0 {
            return RxCompletedUnitFrontier::default();
        }
        let first_index = self.recycle_start.advance(observed);
        if self.descriptors[first_index.get()].word0().rx_done() {
            // The caller only needs one ownership proof. Avoid walking the
            // rest of a terminal-only burst merely to truncate its unit count
            // back to one: the next finite cursor check will prove the next
            // descriptor after this unit has been recycled.
            return RxCompletedUnitFrontier {
                unit_count: 1,
                descriptor_count: 1,
            };
        }
        if !nonterminal_consumed(first_index.get()) {
            return RxCompletedUnitFrontier::default();
        }
        // A split unit still needs a bounded walk through its terminal. This
        // is identical to the general scanner, which already reports at most
        // one unit when the first descriptor is non-terminal.
        for step in 1..descriptor_limit.min(64) {
            let index = first_index.advance(step);
            if self.observed_mask.contains(index.get()) {
                break;
            }
            if self.descriptors[index.get()].word0().rx_done() {
                return RxCompletedUnitFrontier {
                    unit_count: 1,
                    descriptor_count: step + 1,
                };
            }
            if !nonterminal_consumed(index.get()) {
                break;
            }
        }
        RxCompletedUnitFrontier::default()
    }

    /// Transfer one complete RX unit at the recycle frontier exactly once.
    ///
    /// The unit ends at or before the LAST of `cursor`, inclusive
    /// ([`Self::frozen_cursor_descriptor_limit`]): the walker no longer
    /// writes any of its descriptors. A cursor of another ring, live epoch or
    /// generation takes nothing. A saturated producer cannot extend this
    /// ownership transfer to a later completion epoch: that needs a later
    /// cursor.
    pub(crate) fn take_completed_unit_owned<F, B>(
        &mut self,
        cursor: RxFrozenCursor,
        nonterminal_consumed: F,
        reserved: &B,
    ) -> Result<Option<RxCompletedUnit>, RxRingError>
    where
        F: FnMut(usize) -> bool,
        B: RxReservedBuffer,
    {
        let Some((descriptor_limit, _)) = self.frozen_cursor_descriptor_limit(cursor) else {
            return Ok(None);
        };
        let Some(mut unit) =
            self.take_completed_unit_inner(descriptor_limit, nonterminal_consumed)?
        else {
            return Ok(None);
        };
        if !self.reserved_left {
            unit.continues_reserved_landing = self.leave_reserved(reserved)?;
        }
        Ok(Some(unit))
    }

    fn take_completed_unit_inner<F>(
        &mut self,
        descriptor_limit: usize,
        mut nonterminal_consumed: F,
    ) -> Result<Option<RxCompletedUnit>, RxRingError>
    where
        F: FnMut(usize) -> bool,
    {
        if COUNT == 0
            || COUNT > RX_OBSERVED_MASK_CAPACITY
            || descriptor_limit == 0
            || descriptor_limit > COUNT
        {
            return Ok(None);
        }
        let observed = self.observed_prefix_len();
        if observed == COUNT || descriptor_limit > COUNT - observed {
            return Ok(None);
        }
        let first_index = self.recycle_start.advance(observed);
        if self.observed_mask.contains(first_index.get()) {
            return Ok(None);
        }
        let first_word0 = self.validate_descriptor_image(first_index)?;
        let first_length = u32::from(first_word0.length());
        if first_word0.rx_done() {
            let descriptor_address = descriptor_address(self.descriptor_base, first_index.get())?;
            let segment_length = u16::try_from(first_length).map_err(|_| RxRingError::Size)?;
            self.observed_mask.insert(first_index.get());
            return Ok(Some(RxCompletedUnit {
                head_index: first_index.get(),
                descriptor_count: 1,
                descriptor_address,
                staged_word0: staged_word0(first_word0, first_length),
                total_length: first_length as usize,
                segment_lengths: RxCompletedUnitLengths::Single(segment_length),
                continues_reserved_landing: false,
            }));
        }

        let mut segment_lengths = [0_u16; 64];
        let first_consumed = nonterminal_consumed(first_index.get());
        let first_segment_length = if first_consumed { first_length } else { 0 };
        segment_lengths[0] = u16::try_from(first_segment_length).map_err(|_| RxRingError::Size)?;
        let mut total_length = first_segment_length as usize;
        let mut metadata_word0 = first_word0;
        let mut metadata_index = first_index;
        for step in 1..descriptor_limit.min(64) {
            let index = first_index.advance(step);
            if self.observed_mask.contains(index.get()) {
                return Ok(None);
            }
            let word0 = match self.validate_descriptor_image(index) {
                Ok(word0) => word0,
                Err(error) => {
                    self.require_reset();
                    return Err(error);
                }
            };
            let consumed = word0.rx_done() || nonterminal_consumed(index.get());
            let length = if consumed {
                u32::from(word0.length())
            } else {
                0
            };
            if total_length == 0 && consumed {
                metadata_word0 = word0;
                metadata_index = index;
            }
            segment_lengths[step] = u16::try_from(length).map_err(|_| RxRingError::Size)?;
            total_length = total_length
                .checked_add(length as usize)
                .ok_or(RxRingError::Overflow)?;
            if !word0.rx_done() {
                continue;
            }
            let descriptor_count = step + 1;
            // A chain longer than the descriptor length field is received
            // air input, not a ring fault: it is owned and returned like
            // every complete unit, so its consumer can drop and recycle it.
            let staged = u32::try_from(total_length)
                .ok()
                .and_then(|length| staged_word0(metadata_word0, length));
            let descriptor_address =
                descriptor_address(self.descriptor_base, metadata_index.get())?;
            self.observed_mask
                .insert_group::<COUNT>(first_index, descriptor_count);
            return Ok(Some(RxCompletedUnit {
                head_index: first_index.get(),
                descriptor_count,
                descriptor_address,
                staged_word0: staged,
                total_length,
                segment_lengths: RxCompletedUnitLengths::Chained(segment_lengths),
                continues_reserved_landing: false,
            }));
        }
        Ok(None)
    }

    crate::place_rx_hot_path! {
    /// Takes one newly completed descriptor exactly once for this ring epoch.
    ///
    /// Kept in internal SRAM for PSRAM-code profiles: this is invoked once for
    /// every descriptor slot on every receive poll. Hardware runs at HE20
    /// showed that executing the complete poll/copy path from PSRAM capped
    /// useful UDP RX near 65 Mbit/s.
    #[inline(never)]
    pub(crate) fn take_completed_owned<B: RxReservedBuffer>(
        &mut self,
        index: usize,
        reserved: &B,
    ) -> Option<RxCompletedDescriptor> {
        if index >= COUNT {
            return None;
        }
        if self.observed_mask.contains(index) {
            return None;
        }
        let descriptor = &self.descriptors[index];
        let word0 = descriptor.word0();
        if !word0.rx_done() {
            return None;
        }
        self.validate_descriptor_image(RingIndex::checked(index)?).ok()?;
        let descriptor_address = descriptor_address(self.descriptor_base, index).ok()?;
        if !self.reserved_left && self.leave_reserved(reserved).is_err() {
            self.require_reset();
            return None;
        }
        self.observed_mask.insert(index);
        Some(RxCompletedDescriptor {
            index,
            descriptor_address,
            word0,
            next_descriptor_address: descriptor.next_address(),
        })
    }}

    pub(crate) fn validate_completed_descriptor(&self, index: usize) -> Result<bool, RxRingError> {
        let descriptor = self.descriptors.get(index).ok_or(RxRingError::Count)?;
        let word0 = descriptor.word0();
        if !word0.rx_done() {
            return Ok(false);
        }
        self.validate_descriptor_image(RingIndex::checked(index).ok_or(RxRingError::Count)?)?;
        Ok(true)
    }

    fn validate_descriptor_image(
        &self,
        index: RingIndex<COUNT>,
    ) -> Result<WifiRxDmaDescriptorControl, RxRingError> {
        let descriptor = self
            .descriptors
            .get(index.get())
            .ok_or(RxRingError::Count)?;
        let word0 = descriptor.word0();
        let expected_size = u32::from(self.buffer_size);
        if u32::from(word0.size()) != expected_size
            || u32::from(word0.length()) > expected_size
            || descriptor.buffer_address()
                != self
                    .bound_buffer_address(index.get())
                    .ok_or(RxRingError::Count)?
        {
            return Err(RxRingError::Corrupt);
        }
        let terminal_tail = self.pending_tail().unwrap_or(self.accepted_tail);
        let expected_next = if index == terminal_tail {
            0
        } else {
            descriptor_address(self.descriptor_base, index.advance(1).get())?
        };
        if descriptor.next_address() != expected_next {
            return Err(RxRingError::Corrupt);
        }
        Ok(word0)
    }

    crate::place_rx_hot_path! {
    /// Settles a prior append and, when the next half is entirely CPU-owned,
    /// rearms and appends it without stopping the live walker.
    ///
    /// This is the allocation-free Rust ownership form of the recovered
    /// `wDevCtrl.head/tail` transaction. The future tail is kept private until
    /// RX_CONTROL bit 0 self-clears. If the walker exhausted the old frontier
    /// during reload, the doorbell is rung once more before the new tail
    /// becomes accepted.
    #[inline(never)]
    pub(crate) fn recycle_completed_half_owned<M, F>(
        &mut self,
        mmio: &mut M,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        self.recycle_completed_group(
            mmio,
            COUNT / 2,
            RxRecycleGroupShape::EveryDescriptorTerminal,
            prepare_buffer,
        )
    }}

    crate::place_rx_hot_path! {
    /// Rearm the longest currently completed prefix, up to `MAX_BATCH`.
    ///
    /// Unlike the half-ring operation, this does not wait for a fixed-size
    /// group. It is the closer ownership model for the complete
    /// vendor receive path:
    ///
    /// - `wDev_ProcessFiq` posts PP event `0x19`;
    /// - `ppTask` calls `wdevProcessRxSucDataAll`;
    /// - that walker counts the descriptors in one completed RX unit;
    /// - `wDev_DiscardFrame` immediately transfers `(head, tail, count)` to
    ///   `wDev_AppendRxBlocks`.
    ///
    /// The complete vendor archive places `wDev_AppendRxBlocks` in
    /// `wdev.o:.wifislprxiram.55`, `wDev_DiscardFrame` in
    /// `.wifislpiram.18`, and `wdevProcessRxSucDataAll` in
    /// `.wifislprxiram.28`. This proves that prompt reclaim is an intentional
    /// SRAM-resident software path rather than a background hardware feature.
    ///
    /// Hardware provides completion, last/next and the reload doorbell. The
    /// variable-size reclaim policy is software, not automatic DMA recycling.
    /// `MAX_BATCH` bounds one append transaction without imposing a minimum.
    #[inline(never)]
    pub(crate) fn recycle_completed_prefix_owned<const MAX_BATCH: usize, M, F>(
        &mut self,
        mmio: &mut M,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        if MAX_BATCH == 0 || MAX_BATCH > COUNT {
            return Err(RxRingError::Count);
        }
        if mmio.try_with_reload_settled(|_settled| ()).is_none() {
            return Ok(None);
        }
        self.settle_reload(mmio)?;
        if self.reload_pending() {
            return Ok(None);
        }

        let mut completed = 0;
        while completed < MAX_BATCH {
            let index = self.recycle_start.advance(completed);
            if !self.observed_mask.contains(index.get()) || !self.descriptors[index.get()].word0().rx_done() {
                break;
            }
            completed += 1;
        }
        if completed == 0 {
            return Ok(None);
        }
        self.recycle_completed_group(
            mmio,
            completed,
            RxRecycleGroupShape::EveryDescriptorTerminal,
            prepare_buffer,
        )
    }}

    /// Rearm and append one observed RX unit, preserving a multi-descriptor
    /// unit's `not-done .. done` completion shape until all of its bytes have
    /// been copied to independent storage.
    pub(crate) fn recycle_completed_unit_owned<M, F>(
        &mut self,
        mmio: &mut M,
        descriptor_count: usize,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        if descriptor_count == 0 || descriptor_count > COUNT {
            return Err(RxRingError::Count);
        }
        self.recycle_completed_group(
            mmio,
            descriptor_count,
            RxRecycleGroupShape::OneCompletedUnit,
            prepare_buffer,
        )
    }

    /// Rearm an exact contiguous prefix of independently released terminal
    /// descriptors.
    ///
    /// Unlike [`Self::recycle_completed_prefix_owned`], the caller has already
    /// bounded `descriptor_count` by an upper-owner release state. This keeps
    /// a later still-retained descriptor out of the same irreversible append
    /// transaction.
    pub(crate) fn recycle_released_terminal_prefix_owned<M, F>(
        &mut self,
        mmio: &mut M,
        descriptor_count: usize,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        if descriptor_count == 0 || descriptor_count > COUNT {
            return Err(RxRingError::Count);
        }
        self.recycle_completed_group(
            mmio,
            descriptor_count,
            RxRecycleGroupShape::EveryDescriptorTerminal,
            prepare_buffer,
        )
    }

    /// Rearm exactly one copied RX unit released by a frozen vendor LAST.
    ///
    /// `wdevProcessRxSucDataAll` does not collect multiple complete units into
    /// one returned chain. It counts one unit, transfers that exact
    /// `head..tail` chain through `wDev_DiscardFrame`/`wDev_AppendRxBlocks`,
    /// and only then refreshes LAST before examining the next software head.
    /// The affine recycle unit retains the mutable ring loan from its
    /// frozen-LAST observation through this append. A copied cursor or count
    /// cannot authorize a later descriptor generation.
    pub(crate) fn recycle_proven_unit_owned<M, F>(
        &mut self,
        mmio: &mut M,
        descriptor_count: usize,
        prepare_buffer: F,
    ) -> Result<RxLiveAppend<COUNT>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        // Only RxDmaRecycleUnit calls this: its mutable ring loan retains
        // the observed group and the frozen LAST generation until append.
        debug_assert!(
            self.observed_mask
                .contains_group::<COUNT>(self.recycle_start, descriptor_count)
        );
        self.recycle_completed_group(
            mmio,
            descriptor_count,
            RxRecycleGroupShape::OneCompletedUnitThroughFrozenLast,
            prepare_buffer,
        )?
        .ok_or(RxRingError::ReloadPending)
    }

    fn recycle_completed_group<M, F>(
        &mut self,
        mmio: &mut M,
        requested_group_size: usize,
        shape: RxRecycleGroupShape,
        mut prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        if mmio.try_with_reload_settled(|_settled| ()).is_none() {
            return Ok(None);
        }
        self.settle_reload(mmio)?;
        if self.reload_pending() {
            // The settle rang the doorbell once more; the next pass settles
            // that ring before anything else is appended.
            return Ok(None);
        }

        if !self
            .observed_mask
            .contains_group::<COUNT>(self.recycle_start, requested_group_size)
        {
            return Ok(None);
        }
        let accepted_distance = self.recycle_start.distance_to(self.accepted_tail);
        if requested_group_size - 1 > accepted_distance {
            // A completed-looking descriptor beyond the accepted hardware
            // tail is not part of the live list. Reject the complete
            // transaction before modifying buffer guards or descriptor words.
            self.require_reset();
            return Err(RxRingError::Corrupt);
        }
        // The accepted tail is the doorbell's link point: the walker parks
        // on it (LAST) when the list runs out, and the reload doorbell with
        // NEXT = 0 resumes it at `accepted_tail.next`. A group that covers
        // the accepted tail, the whole ring, cannot be appended behind
        // itself. Independent terminal descriptors stop one short: the old
        // tail stays observed and rejoins in a later group, once the walker
        // has moved past it (the later-frontier link-release proof). One
        // unit spanning the whole ring is no frame
        // ([`rx_ring_outspans_any_unit`]) and fails closed.
        let group_size = if requested_group_size - 1 == accepted_distance {
            match shape {
                RxRecycleGroupShape::EveryDescriptorTerminal => requested_group_size - 1,
                RxRecycleGroupShape::OneCompletedUnit
                | RxRecycleGroupShape::OneCompletedUnitThroughFrozenLast => {
                    self.require_reset();
                    return Err(RxRingError::Corrupt);
                }
            }
        } else {
            requested_group_size
        };
        if group_size == 0 {
            return Ok(None);
        }

        for step in 0..group_size {
            let index = self.recycle_start.advance(step);
            let word0 = match self.validate_descriptor_image(index) {
                Ok(word0) => word0,
                Err(error) => {
                    self.require_reset();
                    return Err(error);
                }
            };
            let valid_completion_shape = match shape {
                RxRecycleGroupShape::EveryDescriptorTerminal => word0.rx_done(),
                RxRecycleGroupShape::OneCompletedUnit
                | RxRecycleGroupShape::OneCompletedUnitThroughFrozenLast => {
                    word0.rx_done() == (step + 1 == group_size)
                }
            };
            if !valid_completion_shape {
                self.require_reset();
                return Err(RxRingError::Corrupt);
            }
            // Validate every value needed by the later publication before a
            // buffer guard is modified. Hardware is already excluded from
            // the observed group, so these images remain stable until rearm.
            rx_rearm_word(word0).ok_or(RxRingError::Size)?;
            if step + 1 < group_size {
                descriptor_address(self.descriptor_base, index.advance(1).get())?;
            }
        }
        let head_index = self.recycle_start;
        let head_address = descriptor_address(self.descriptor_base, head_index.get())?;
        let tail_index = head_index.advance(group_size - 1);
        if self.descriptors[self.accepted_tail.get()].next_address() != 0 {
            // The accepted tail is the only legal append point. Validate it
            // before the first irreversible buffer/descriptor mutation so a
            // corrupt list cannot become a partially rearmed live arena.
            self.require_reset();
            return Err(RxRingError::Corrupt);
        }
        // The frozen-LAST path validates its generation-specific cursor proof
        // before entering this mutation routine. Every other path still needs
        // a fresh ownership observation here.
        if !matches!(
            shape,
            RxRecycleGroupShape::OneCompletedUnitThroughFrozenLast
        ) && !self.observe_current_completed_unit_link_release(mmio, group_size)
        {
            return Ok(None);
        }
        let Some(next_cursor_generation) = self.cursor_generation.checked_add(1) else {
            self.require_reset();
            return Err(RxRingError::Corrupt);
        };
        for step in 0..group_size {
            let index = self.recycle_start.advance(step);
            if let Err(error) = prepare_buffer(index.get()) {
                // The completed-unit token has already been consumed and an
                // earlier callback may have changed buffer-side guards. That
                // partial transaction cannot be reconstructed from the next
                // service pass, so fail closed instead of leaving a silent
                // observed-mask dead-end.
                self.require_reset();
                return Err(error);
            }
        }
        for step in 0..group_size {
            let index = self.recycle_start.advance(step);
            let descriptor = &self.descriptors[index.get()];
            let next = if step + 1 < group_size {
                descriptor_address(self.descriptor_base, index.advance(1).get())?
            } else {
                0
            };
            let buffer_address = self
                .bound_buffer_address(index.get())
                .ok_or(RxRingError::Count)?;
            // SAFETY: the live ring's descriptor base and buffer table are the
            // ones `prepare_inner` took from its placed storage arena: that
            // arena's descriptors and buffers, which the link keeps in the DMA
            // window.
            #[allow(unsafe_code, reason = "typed addresses of the placed arena")]
            let (buffer, next) = unsafe {
                (
                    dma_address::placed(buffer_address),
                    dma_address::placed_link(next),
                )
            };
            descriptor.publish_owned(
                rx_rearm_word(descriptor.word0()).ok_or(RxRingError::Size)?,
                buffer,
                next,
            );
        }

        let accepted_tail = &self.descriptors[self.accepted_tail.get()];
        // This type is the sole publication authority. All descriptors in the
        // appended group were observed complete, rearmed and remain
        // unreachable until this old-tail link and the following doorbell.
        // SAFETY: as for the group above, `head_address` names a descriptor
        // of the live ring's placed table.
        #[allow(unsafe_code, reason = "a link to a descriptor of the placed table")]
        let head = unsafe { dma_address::placed_link(head_address) };
        accepted_tail.publish_next(head);
        mmio.fence();
        mmio.request_reload(&self.binding);
        mmio.fence();

        self.cursor_generation = next_cursor_generation;
        self.pending_tail_plus_one = tail_index.get() + 1;
        self.reload_rerung = false;
        self.observed_mask
            .remove_group::<COUNT>(self.recycle_start, group_size);
        self.recycle_start = self.recycle_start.advance(group_size);
        Ok(Some(RxLiveAppend {
            head_index,
            head_address,
            tail_index,
            descriptor_count: group_size,
        }))
    }

    pub const fn observed_mask(&self) -> RxObservedMask {
        self.observed_mask
    }

    pub(crate) fn observed_prefix_len(&self) -> usize {
        observed_prefix_len::<COUNT>(self.observed_mask, self.recycle_start)
    }

    /// Whether every descriptor in this ring epoch has returned to software.
    pub const fn all_observed(&self) -> bool {
        self.observed_mask.count_ones() as usize == COUNT
    }

    pub const fn recycle_start(&self) -> RingIndex<COUNT> {
        self.recycle_start
    }

    pub const fn accepted_tail(&self) -> RingIndex<COUNT> {
        self.accepted_tail
    }

    /// Number of descriptors in the accepted hardware list beginning at one
    /// instantaneous `RX_NEXT_DESCRIPTOR` observation.
    ///
    /// This is pressure telemetry, not an ownership proof. Hardware may
    /// advance immediately after the observation, so callers must not use the
    /// returned value to authorize descriptor reads, mutation or recycle. A
    /// zero complete NEXT word means the finite list is exhausted; an address
    /// outside this arena is reported as unknown instead of being projected
    /// onto a plausible ring slot.
    pub fn accepted_list_remaining_from_next(
        &self,
        next: crate::ieee80211::dma::rx_dma::RxDmaNextDescriptor,
    ) -> Option<usize> {
        if next.is_zero() {
            return Some(0);
        }
        let next_index = descriptor_index::<COUNT>(next.address_low(), self.descriptor_base)?;
        Some(next_index.distance_to(self.accepted_tail) + 1)
    }

    /// Freeze one ordered LAST/NEXT image for this descriptor generation.
    ///
    /// A NEXT naming a detached descriptor fails closed
    /// ([`RxReloadEvidence::walker_on_detached`]).
    pub fn freeze_cursor<M: RxDma>(&mut self, mmio: &mut M) -> Result<RxFrozenCursor, RxRingError> {
        let (last_descriptor_low, next_descriptor_low) = mmio.with_ordered_cursor(|cursor| {
            (cursor.last_descriptor_low(), cursor.next_descriptor_low())
        });
        // Make both MMIO observations precede descriptor and payload reads by
        // the caller which receives this generation-bound snapshot.
        mmio.fence();
        // No false positive on a just-completed descriptor. A descriptor
        // is detached only by `RxDmaCompletedUnit::detach_single`, whose
        // `detached_word` requires it observed and RX_DONE. Its unit came
        // from `RxDmaStorage::take_completed_unit`, which requires an
        // `RxFrozenCursor` and ends the unit at that cursor's LAST inclusive
        // (`frozen_cursor_descriptor_limit`); a cursor of another ring, live
        // epoch or generation takes nothing. So every detached descriptor
        // lies at or before a LAST sampled before its detach: the walker
        // completed it. LAST names the last completed descriptor and NEXT
        // the one to fill after it, `LAST.next` or zero
        // (SOURCE[HIL_W1_OWNER_BIT_2026_10_18]: stopped at NEXT = P with
        // LAST = P - 1; H1 in HIL_W1_RX_WALKER_2026_10_18: NEXT =
        // LAST.next), so a NEXT read after that LAST never names it again.
        // NEXT may hold a descriptor whose RX_DONE is already visible while
        // LAST has not reached it; that unit is outside the LAST bound, not
        // taken, and keeps its owner bit. No relink names a detached
        // descriptor: a rearm in `recycle_completed_group` sets the owner
        // bit and leaves the observed mask, and a group's tail link is zero
        // until the accepted tail links the rearmed head. The frozen-LAST
        // unit at exhaustion is detached with NEXT = 0, which names no
        // descriptor (`names_detached`).
        self.check_walker_on_detached(next_descriptor_low)?;
        Ok(RxFrozenCursor {
            last_descriptor_low,
            next_descriptor_low,
            descriptor_base: self.descriptor_base,
            arena_identity: self.descriptors.as_ptr() as usize,
            live_epoch: self.arena.live_epoch(),
            generation: self.cursor_generation,
        })
    }

    /// Return whether an exhausted finite hardware list still owes software
    /// the accepted terminal descriptor's writeback.
    ///
    /// RxDescriptor writeback can become visible after this cursor edge and no
    /// later RX-success IRQ is guaranteed. Callers use the result only to keep
    /// a cooperative ownership probe alive; it never authorizes rearm.
    pub fn exhausted_terminal_writeback_pending<M: RxDma>(&mut self, mmio: &mut M) -> bool {
        if self.observed_mask.contains(self.accepted_tail.get()) {
            return false;
        }
        mmio.with_ordered_cursor(|cursor| {
            if cursor.next_descriptor_low() != 0 {
                return false;
            }
            descriptor_index::<COUNT>(cursor.last_descriptor_low(), self.descriptor_base)
                == Some(self.accepted_tail)
        })
    }

    pub const fn reload_pending(&self) -> bool {
        self.pending_tail_plus_one != 0
    }

    const fn pending_tail(&self) -> Option<RingIndex<COUNT>> {
        match self.pending_tail_plus_one.checked_sub(1) {
            Some(index) => RingIndex::checked(index),
            None => None,
        }
    }

    /// Whether a completed nonterminal descriptor still awaits proof that the
    /// walker latched its old link before software may rewrite it for recycle.
    pub const fn completion_release_probe_pending(&self) -> bool {
        self.completion_release_probe_pending
    }

    /// Prove that hardware no longer needs the terminal descriptor's current
    /// `next` word before a completed unit is taken for recycle.
    ///
    /// `RX_DONE`, LAST and even NEXT naming the old successor can all become
    /// visible before the walker has fetched the completed descriptor's link.
    /// Hardware reproduced that ordering after two identical LAST/NEXT samples, so
    /// neither sampling nor elapsed time is an ownership proof. A nonterminal
    /// link is released only when LAST advances through a later terminal-
    /// complete descriptor. A zero successor is the real exhausted-list
    /// terminal and deliberately needs no later frontier.
    fn observe_completed_unit_link_release_from_snapshot(
        &mut self,
        last_descriptor_low: u32,
        next_descriptor_low: u32,
        descriptor_count: usize,
    ) -> bool {
        self.completed_unit_link_released_from_ordered_snapshot(
            last_descriptor_low,
            next_descriptor_low,
            descriptor_count,
        )
    }

    /// Sample the hardware-owned LAST/NEXT pair and prove release of the
    /// current completed unit's link word.
    ///
    /// Production callers deliberately cannot supply the LAST value: a bare
    /// integer is not ownership evidence and could otherwise cache a forged
    /// release which a later storage-bound recycle would trust.
    pub fn observe_current_completed_unit_link_release<M: RxDma>(
        &mut self,
        mmio: &mut M,
        descriptor_count: usize,
    ) -> bool {
        mmio.with_ordered_cursor(|cursor| {
            self.observe_completed_unit_link_release_from_snapshot(
                cursor.last_descriptor_low(),
                cursor.next_descriptor_low(),
                descriptor_count,
            )
        })
    }

    /// Whether `cursor` was frozen on this ring in its current live epoch and
    /// descriptor generation.
    fn frozen_cursor_current(&self, cursor: RxFrozenCursor) -> bool {
        cursor.descriptor_base == self.descriptor_base
            && cursor.arena_identity == self.descriptors.as_ptr() as usize
            && cursor.live_epoch == self.arena.live_epoch()
            && cursor.generation == self.cursor_generation
    }

    /// Whether a frozen vendor-style LAST snapshot releases the current unit.
    ///
    /// `wdevProcessRxSucDataAll` mutates and returns the descriptor equal to
    /// its saved LAST before taking another snapshot. Keep this proof separate
    /// from the fallback live-cursor probe: it is valid only while the caller
    /// has not appended anything since freezing `last_descriptor_low`.
    pub(crate) fn frozen_cursor_releases_unit(
        &self,
        cursor: RxFrozenCursor,
        descriptor_count: usize,
    ) -> bool {
        if descriptor_count == 0 || descriptor_count > COUNT {
            return false;
        }
        if !self.frozen_cursor_current(cursor) {
            return false;
        }
        let Some(last_index) =
            descriptor_index::<COUNT>(cursor.last_descriptor_low, self.descriptor_base)
        else {
            return false;
        };
        let tail_distance = descriptor_count - 1;
        let last_distance = self.recycle_start.distance_to(last_index);
        let accepted_distance = self.recycle_start.distance_to(self.accepted_tail);
        tail_distance <= last_distance && last_distance <= accepted_distance
    }

    fn completed_unit_link_released_from_ordered_snapshot(
        &mut self,
        last_descriptor_low: u32,
        next_descriptor_low: u32,
        descriptor_count: usize,
    ) -> bool {
        if descriptor_count == 0 || descriptor_count > COUNT {
            self.completion_release_probe_pending = false;
            return false;
        }
        let tail_index = self.recycle_start.advance(descriptor_count - 1);
        let Some(last_index) = descriptor_index::<COUNT>(last_descriptor_low, self.descriptor_base)
        else {
            self.completion_release_probe_pending = false;
            return false;
        };
        let tail_next_low =
            self.descriptors[tail_index.get()].next_address() & RX_DESCRIPTOR_ADDRESS_LOW_MASK;
        let tail_distance = descriptor_count - 1;
        let last_distance = self.recycle_start.distance_to(last_index);
        let accepted_distance = self.recycle_start.distance_to(self.accepted_tail);
        let later_completed_frontier = last_distance > tail_distance
            && last_distance <= accepted_distance
            && (tail_distance + 1..=last_distance).all(|distance| {
                self.descriptors[self.recycle_start.advance(distance).get()]
                    .word0()
                    .rx_done()
            });
        // A zero link describes the software topology, not the walker's
        // current ownership. RX_DONE/LAST may become visible before hardware
        // has consumed that terminal link. Only NEXT=0 proves the finite list
        // is exhausted and permits descriptor word/buffer rearm.
        let exhausted_terminal = tail_next_low == 0 && next_descriptor_low == 0;
        let exhausted_accepted_list = next_descriptor_low == 0
            && last_index == self.accepted_tail
            && tail_distance <= last_distance;
        if exhausted_terminal || exhausted_accepted_list || later_completed_frontier {
            self.completion_release_probe_pending = false;
            return true;
        }
        self.completion_release_probe_pending = true;
        false
    }

    /// Observe one live-append doorbell edge without waiting.
    ///
    /// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS]: the complete ROM body at
    /// `0x2f83_8a7e` spins on `RX_CONTROL.APPEND_DESCRIPTOR_RELOAD` with the
    /// exact `0x186a1` bound, then immediately samples `RX_NEXT_DESCRIPTOR`
    /// to learn whether the walker exhausted the old frontier. Deferring
    /// that sample until after another RX processing pass is observably
    /// unsafe: the appended half can itself reach the terminal descriptor
    /// first, making `RX_LAST_DESCRIPTOR` indistinguishable from the case
    /// that needs no re-ring.
    ///
    /// A settle that rings the doorbell once more leaves the reload
    /// [`Pending`](RxReloadObservation::Pending).
    pub fn poll_pending_reload<M: RxDma>(
        &mut self,
        mmio: &mut M,
    ) -> Result<RxReloadObservation, RxRingError> {
        if self.pending_tail().is_none() {
            return Ok(RxReloadObservation::Settled);
        }
        if mmio.try_with_reload_settled(|_settled| ()).is_none() {
            return Ok(RxReloadObservation::Pending);
        }
        self.settle_reload(mmio)?;
        Ok(if self.reload_pending() {
            RxReloadObservation::Pending
        } else {
            RxReloadObservation::Settled
        })
    }

    /// Complete the reload doorbell and its settle as one unscheduled
    /// transaction.
    ///
    /// `SOURCE[ROM_REV0_WDEV_APPEND_RX_BLOCKS]`: the vendor function performs
    /// the bounded `RX_CONTROL` loop and immediately samples `NEXT`, then
    /// conditional `LAST`, without returning to its scheduler. Yielding after
    /// a pending sample lets a later RX completion replace that cursor epoch.
    ///
    /// The doorbells are observed until they clear or `settle`'s limit has
    /// passed on its clock, counted from the call; the first observation
    /// always runs. When a doorbell is still pending then, the reload stays
    /// pending in this owner and the call returns `ReloadPending`. Each wait
    /// records its observation count in [`RxReloadEvidence::max_reload_attempts`].
    /// A settle rings the doorbell at most once more per appended tail, so
    /// this waits for at most two doorbells.
    pub fn complete_pending_reload<M: RxDma, C: oer_time::Clock + ?Sized>(
        &mut self,
        mmio: &mut M,
        settle: Settle<'_, C>,
    ) -> Result<(), RxRingError> {
        while self.pending_tail().is_some() {
            let mut attempts = 0_u32;
            let settled = crate::settle::settle(settle, || {
                attempts = attempts.saturating_add(1);
                mmio.try_with_reload_settled(|_settled| ())
            });
            record_maximum(&self.arena.max_reload_attempts, attempts);
            settled.map_err(|crate::settle::Unsettled| RxRingError::ReloadPending)?;
            self.settle_reload(mmio)?;
        }
        Ok(())
    }

    /// Accept the pending tail after its doorbell cleared, or ring the
    /// doorbell once more.
    ///
    /// NEXT nonzero: the walker holds a descriptor of the list, which now
    /// ends at the pending tail. NEXT zero with LAST at the pending tail: the
    /// walker consumed the appended group too. NEXT zero with LAST short of
    /// the pending tail: the walker reached the old tail's zero link before
    /// it followed the new one. The vendor republishes `LAST->next` through
    /// `RX_DESCRIPTOR_BASE` there; this owner never writes BASE after the
    /// cold publication and rings the doorbell once more instead: with NEXT
    /// zero the doorbell resumes the walker at `LAST->next`
    /// (HIL_W1_RX_WALKER_2026_10_18). Still zero after that second ring
    /// contradicts that rule and fails closed. A NEXT naming a detached
    /// descriptor fails closed too ([`RxReloadEvidence::walker_on_detached`]).
    fn settle_reload<M: RxDma>(&mut self, mmio: &mut M) -> Result<(), RxRingError> {
        let Some(pending_tail) = self.pending_tail() else {
            return Ok(());
        };
        let mut next_descriptor = crate::ieee80211::dma::rx_dma::RxDmaNextDescriptor::default();
        let mut last_descriptor_low = None;
        let exhausted_short = mmio.with_reload_observation(|observation| {
            next_descriptor = observation.next_descriptor();
            last_descriptor_low = observation.exhausted_last_descriptor_low();
            if !next_descriptor.is_zero() {
                return Ok(false);
            }
            let last_descriptor_low = last_descriptor_low.ok_or(RxRingError::Corrupt)?;
            let last_index = descriptor_index::<COUNT>(last_descriptor_low, self.descriptor_base)
                .ok_or(RxRingError::Corrupt)?;
            Ok(last_index != pending_tail)
        });
        self.record_reload_observation(next_descriptor, last_descriptor_low);
        // The doorbell resumed the walker: it must not hold a detached
        // descriptor. No false positive on a just-completed one: every
        // detached descriptor was taken through a frozen cursor's LAST
        // inclusive (`frozen_cursor_descriptor_limit`), so the walker had
        // completed it and NEXT, which names the descriptor after LAST or
        // zero, has left it for good (the proof next to the check in
        // `freeze_cursor`). A doorbell rung on an exhausted list resumes at
        // `LAST.next` (H1, HIL_W1_RX_WALKER_2026_10_18): the rearmed head
        // that `recycle_completed_group` linked, owner bit set, never
        // observed. With LAST itself detached at exhaustion, a NEXT still
        // zero names no descriptor (`names_detached`).
        self.check_walker_on_detached(next_descriptor.address_low())?;
        let exhausted_short = match exhausted_short {
            Ok(exhausted_short) => exhausted_short,
            Err(error) => {
                self.require_reset();
                return Err(error);
            }
        };
        if !exhausted_short {
            self.accepted_tail = pending_tail;
            self.pending_tail_plus_one = 0;
            self.reload_rerung = false;
            return Ok(());
        }
        if self.reload_rerung {
            increment_saturating(&self.arena.rering_failures);
            self.require_reset();
            return Err(RxRingError::Corrupt);
        }
        // The old tail's link to the appended head was published before the
        // first doorbell and is unchanged; only the doorbell is repeated.
        mmio.fence();
        mmio.request_reload(&self.binding);
        mmio.fence();
        self.reload_rerung = true;
        increment_saturating(&self.arena.reload_rerings);
        Ok(())
    }
}

impl<const COUNT: usize> Drop for RxRingLive<'_, COUNT> {
    fn drop(&mut self) {
        if self.requires_stop {
            RxDmaArenaState::ResetRequired.store(self.arena.lifecycle());
        }
    }
}

/// One descriptor word describing a complete unit of `length` bytes with
/// `word0`'s status bits: size and length both `length`, `RX_DONE` and
/// `OWNER` set. `None` when `length` does not fit the length field.
const fn staged_word0(
    word0: WifiRxDmaDescriptorControl,
    length: u32,
) -> Option<WifiRxDmaDescriptorControl> {
    if length > u16::MAX as u32 {
        return None;
    }
    let length = length as u16;
    match word0.with_size(length) {
        Some(word0) => match word0.with_length(length) {
            Some(word0) => Some(word0.with_rx_done(true).with_owner(true)),
            None => None,
        },
        None => None,
    }
}

fn validate_live_ring_geometry<const COUNT: usize>() -> Result<(), RxRingError> {
    if COUNT < 2 || COUNT > RX_OBSERVED_MASK_CAPACITY || !COUNT.is_multiple_of(2) {
        Err(RxRingError::Count)
    } else {
        Ok(())
    }
}

fn descriptor_address(descriptor_base: u32, index: usize) -> Result<u32, RxRingError> {
    let index = u32::try_from(index).map_err(|_| RxRingError::Overflow)?;
    descriptor_base
        .checked_add(
            index
                .checked_mul(DESCRIPTOR_BYTES)
                .ok_or(RxRingError::Overflow)?,
        )
        .ok_or(RxRingError::Overflow)
}

fn descriptor_snapshot<const COUNT: usize>(
    descriptors: &[RxDescriptor; COUNT],
    descriptor_base: u32,
    index: usize,
) -> Option<RxDescriptorSnapshot> {
    let descriptor = descriptors.get(index)?;
    Some(RxDescriptorSnapshot {
        index,
        address: descriptor_address(descriptor_base, index).ok()?,
        word0: descriptor.word0(),
        buffer_address: descriptor.buffer_address(),
        next_address: descriptor.next_address(),
    })
}

fn descriptor_index_full<const COUNT: usize>(
    address: u32,
    descriptor_base: u32,
) -> Option<RingIndex<COUNT>> {
    let offset = address.checked_sub(descriptor_base)?;
    if offset % DESCRIPTOR_BYTES != 0 {
        return None;
    }
    let index = usize::try_from(offset / DESCRIPTOR_BYTES).ok()?;
    RingIndex::checked(index)
}

fn ring_topology_snapshot<const COUNT: usize>(
    descriptors: &[RxDescriptor; COUNT],
    descriptor_base: u32,
    buffer_addresses: &[u32; COUNT],
    start: RingIndex<COUNT>,
    require_armed: bool,
) -> RxRingTopologySnapshot {
    let tail_index = start.previous();
    let head_address = descriptor_address(descriptor_base, start.get()).unwrap_or(0);
    let tail_address = descriptor_address(descriptor_base, tail_index.get()).unwrap_or(0);
    let mut snapshot = RxRingTopologySnapshot {
        descriptor_base,
        start_index: start.get(),
        head_address,
        head_next_address: 0,
        tail_index: tail_index.get(),
        tail_address,
        visited_descriptors: 0,
        terminal_descriptors: 0,
        valid: false,
    };
    if validate_live_ring_geometry::<COUNT>().is_err() {
        return snapshot;
    }

    let mut visited = RxObservedMask::default();
    let mut index = start;
    for step in 0..COUNT {
        if visited.contains(index.get()) {
            return snapshot;
        }
        visited.insert(index.get());
        snapshot.visited_descriptors += 1;

        let descriptor = &descriptors[index.get()];
        let word0 = descriptor.word0();
        let buffer_address = descriptor.buffer_address();
        let next_address = descriptor.next_address();
        if step == 0 {
            snapshot.head_next_address = next_address;
        }
        if buffer_address != buffer_addresses[index.get()] {
            return snapshot;
        }
        if require_armed {
            let capacity = u32::from(word0.size());
            if capacity == 0
                || u32::from(word0.length()) != capacity
                || !word0.owner()
                || word0.bit_29_opaque()
                || word0.rx_done()
            {
                return snapshot;
            }
        }

        if next_address == 0 {
            snapshot.terminal_descriptors += 1;
            if step + 1 != COUNT || index != tail_index {
                return snapshot;
            }
            snapshot.valid =
                snapshot.visited_descriptors == COUNT && snapshot.terminal_descriptors == 1;
            return snapshot;
        }
        if step + 1 == COUNT {
            return snapshot;
        }
        let Some(next_index) = descriptor_index_full::<COUNT>(next_address, descriptor_base) else {
            return snapshot;
        };
        let expected_index = index.advance(1);
        if next_index != expected_index {
            return snapshot;
        }
        index = next_index;
    }
    snapshot
}

fn descriptor_index<const COUNT: usize>(
    low_address: u32,
    descriptor_base: u32,
) -> Option<RingIndex<COUNT>> {
    let base_low = descriptor_base & RX_DESCRIPTOR_ADDRESS_LOW_MASK;
    let offset = low_address.checked_sub(base_low)?;
    if offset % DESCRIPTOR_BYTES != 0 {
        return None;
    }
    let index = usize::try_from(offset / DESCRIPTOR_BYTES).ok()?;
    RingIndex::checked(index)
}

/// Count a contiguous observed prefix in ring order without rescanning every
/// already-observed descriptor on each unit transfer.
///
/// At most four native-word chunks cover a wrapping 96-slot ring. Each
/// chunk uses a bit scan, preserving arbitrary holes used by the raw model
/// without introducing a per-descriptor scan in the production path.
fn observed_prefix_len<const COUNT: usize>(
    observed_mask: RxObservedMask,
    start: RingIndex<COUNT>,
) -> usize {
    if COUNT > RX_OBSERVED_MASK_CAPACITY {
        return 0;
    }
    let mut cursor = start;
    let mut remaining = COUNT;
    let mut prefix = 0_usize;
    while remaining != 0 {
        let offset = cursor.get() % 32;
        let chunk = remaining.min(32 - offset).min(COUNT - cursor.get());
        let bits = observed_mask.words[cursor.get() / 32] >> offset;
        let chunk_mask = if chunk == 32 {
            u32::MAX
        } else {
            (1_u32 << chunk) - 1
        };
        let missing = !bits & chunk_mask;
        if missing != 0 {
            return prefix + missing.trailing_zeros() as usize;
        }
        prefix += chunk;
        remaining -= chunk;
        cursor = cursor.advance(chunk);
    }
    prefix
}

fn relink_rotated_ring<const COUNT: usize>(
    descriptors: &[RxDescriptor; COUNT],
    descriptor_base: u32,
    buffer_addresses: &[u32; COUNT],
    start: RingIndex<COUNT>,
) -> Result<(), RxRingError> {
    for step in 0..COUNT {
        let index = start.advance(step);
        let next = if step + 1 < COUNT {
            descriptor_address(descriptor_base, index.advance(1).get())?
        } else {
            0
        };
        let word0 = descriptors[index.get()].word0();
        // SAFETY: only `prepare_inner` calls this, with the descriptor base
        // and the buffer table of its placed storage arena.
        #[allow(unsafe_code, reason = "typed addresses of the placed arena")]
        let (buffer, next) = unsafe {
            (
                dma_address::placed(buffer_addresses[index.get()]),
                dma_address::placed_link(next),
            )
        };
        descriptors[index.get()].publish_owned(word0, buffer, next);
    }
    Ok(())
}

/// Restores the two guard words required by the recovered RX recycle path.
///
/// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS] and the source-owned production
/// recycle transaction.
///
/// `buffer` is the complete allocation, including the four-byte trailing
/// guard. `capacity` is the byte count published in the DMA descriptor.
pub fn prepare_recycled_buffer(buffer: &mut [u8], capacity: usize) -> Result<(), RxRingError> {
    if capacity < core::mem::size_of::<u32>()
        || capacity
            .checked_add(core::mem::size_of::<u32>())
            .is_none_or(|required| required > buffer.len())
    {
        return Err(RxRingError::Size);
    }
    let sentinel = RX_BUFFER_SENTINEL.to_le_bytes();
    buffer[..sentinel.len()].copy_from_slice(&sentinel);
    buffer[capacity..capacity + sentinel.len()].copy_from_slice(&sentinel);
    Ok(())
}

/// Builds the cold, zero-terminated list used by the recovered S31 RX path.
fn build_cold_ring_inner(
    descriptors: &[RxDescriptor],
    descriptor_dma_base: u32,
    buffer_addresses: &[u32],
    buffer_size: u32,
) -> Result<(), RxRingError> {
    if descriptors.is_empty() {
        return Err(RxRingError::Empty);
    }
    if descriptors.len() != buffer_addresses.len() {
        return Err(RxRingError::Count);
    }
    let word0 = rx_armed_word(buffer_size).ok_or(RxRingError::Size)?;
    let count = u32::try_from(descriptors.len()).map_err(|_| RxRingError::Overflow)?;
    let span = count
        .checked_mul(DESCRIPTOR_BYTES)
        .ok_or(RxRingError::Overflow)?;
    // The links below add descriptor offsets to the base.
    let _end = descriptor_dma_base
        .checked_add(span)
        .ok_or(RxRingError::Overflow)?;
    for (index, descriptor) in descriptors.iter().enumerate() {
        let next = if index + 1 < descriptors.len() {
            descriptor_dma_base + (index as u32 + 1) * DESCRIPTOR_BYTES
        } else {
            0
        };
        // SAFETY: only `prepare_inner` calls this, with the descriptor base
        // and the buffer table of its placed storage arena
        // (`PlacedRxDma`): the link keeps every one of them in the DMA
        // window, and the storage stays allocated for the DMA epoch.
        #[allow(unsafe_code, reason = "typed addresses of the placed arena")]
        let (buffer, next) = unsafe {
            (
                dma_address::placed(buffer_addresses[index]),
                dma_address::placed_link(next),
            )
        };
        descriptor.publish_owned(word0, buffer, next);
    }
    Ok(())
}

fn publish_cold_ring_inner<M: RxDma>(
    mmio: &mut M,
    binding: &RxDmaBinding<'_>,
    arena: &RxArenaWords,
) -> Result<(), RxRingError> {
    mmio.fence();
    mmio.configure_descriptor_window(binding);
    mmio.write_descriptor_base(binding);
    increment_saturating(&arena.base_writes);
    mmio.fence();
    Ok(())
}

/// The reserved descriptor R of `table`, at index `COUNT`.
fn reserved_snapshot<const COUNT: usize>(
    table: &RxDescriptorTable<COUNT>,
    descriptor_base: u32,
) -> RxDescriptorSnapshot {
    let reserved = table.reserved();
    RxDescriptorSnapshot {
        index: COUNT,
        address: descriptor_address(descriptor_base, COUNT).unwrap_or(0),
        word0: reserved.word0(),
        buffer_address: reserved.buffer_address(),
        next_address: reserved.next_address(),
    }
}

/// Opens the RX walker after a cold ring was published.
///
/// The vendor cold path keeps these operations separate:
/// `wDev_AppendRxBlocks` publishes the first descriptor base, while the later
/// `chip_enable` path calls `hal_mac_rx_enable`. Keeping this as a distinct
/// operation preserves that ordering and gives the base register time to
/// settle while the caller completes channel/MAC setup.
fn enable_receive_inner<M: RxDma>(
    mmio: &mut M,
    binding: &RxDmaBinding<'_>,
) -> Result<(), RxRingError> {
    mmio.try_with_walker_enabled(binding, |_enabled| ())
        .ok_or(RxRingError::WalkerStartUnconfirmed)
}

/// Stop the RX walker and confirm that the peripheral released its enable
/// edge before the owner rebuilds descriptor words or links.
///
/// The pinned `hal_mac_rx_disable` body is exactly this bit clear. The fence
/// and readback turn that raw leaf into an explicit Rust ownership boundary:
/// callers may mutate the ring only after this function returns `Ok(())`.
///
/// A walker which does not read back as stopped is `WalkerStopUnconfirmed`.
/// A stop proof naming another arena proves nothing about this one: the arena is
/// poisoned until radio reset and the stop is `Corrupt`.
fn disable_receive_inner<M: RxDma>(
    mmio: &mut M,
    binding: &RxDmaBinding<'_>,
    arena: &RxArenaWords,
) -> Result<(), RxRingError> {
    match mmio.try_with_walker_stopped(binding, |stopped| stopped.names(arena)) {
        None => Err(RxRingError::WalkerStopUnconfirmed),
        Some(true) => Ok(()),
        Some(false) => {
            RxDmaArenaState::ResetRequired.store(arena.lifecycle());
            Err(RxRingError::Corrupt)
        }
    }
}

#[cfg(test)]
mod observed_prefix_tests;
