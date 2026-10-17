//! Typed ownership and publication for the ESP32-S31 RX DMA ring.
//!
//! This module contains descriptor-walker mechanics only. 802.11 frame
//! decoding remains in the MAC crate, while the semantic MMIO operations are
//! defined by [`crate::rx_dma::RxDma`].

#[cfg(not(any(test, feature = "test-support")))]
use core::sync::atomic::AtomicU32;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::ring_index::RingIndex;

use crate::{
    descriptor::{
        BIT_29, BIT_30, BIT_31, DESCRIPTOR_BYTES, Descriptor, LENGTH_MASK, LENGTH_SHIFT, SIZE_MASK,
        descriptor_address_valid, dma_range_valid, length as descriptor_length, rx_armed_word,
        rx_done, rx_rearm_word, size as descriptor_size,
    },
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

const RX_DESCRIPTOR_ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
const RX_EXHAUSTED_REPUBLICATION_ACCEPTED: u32 = 0x8000_0000;
/// Exact maximum number of `APPEND_DESCRIPTOR_RELOAD` observations recovered
/// from `wDev_AppendRxBlocks`.
///
/// The source-owned driver exposes each observation separately so an Embassy
/// integration can yield between them instead of reproducing the ROM spin.
pub const RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT: u32 = 0x0001_86a1;

#[derive(Clone, Copy, Debug)]
pub struct RxSegment<'a> {
    pub descriptor_address: u32,
    pub descriptor_word0: u32,
    pub buffer: &'a [u8],
    pub next_descriptor_address: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxRingError {
    Empty,
    Count,
    Address,
    DescriptorOwnerAddress,
    DescriptorBaseAddress,
    BufferAddress,
    TailBufferAddress,
    Size,
    Overflow,
    Busy,
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
    pub word0: u32,
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

/// Raw observations made while completing the vendor reload-repair suffix.
///
/// These counters deliberately record decisions and field observations without
/// assigning hardware meaning to unknown state. They remain attached to the live ring
/// so a later disconnect can explain whether a direct BASE repair preceded a
/// fault.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxReloadRepairEvidence {
    pub observations: u32,
    pub unknown_upper_with_zero_address: u32,
    pub base_repairs: u32,
    pub last_next_low: u32,
    pub last_last_low: Option<u32>,
    pub last_repair_head: Option<u32>,
}

// Production has one hardware RX-ring authority. Keep
// qualification evidence outside the movable ownership token: adding even a
// few words to that token is multiplied across nested async state machines and
// measurably changes the stack shape being diagnosed. Native model tests keep
// evidence per instance so parallel tests cannot race through these globals.
#[cfg(not(any(test, feature = "test-support")))]
static RELOAD_OBSERVATIONS: AtomicU32 = AtomicU32::new(0);
#[cfg(not(any(test, feature = "test-support")))]
static RELOAD_UPPER_ONLY: AtomicU32 = AtomicU32::new(0);
#[cfg(not(any(test, feature = "test-support")))]
static RELOAD_BASE_REPAIRS: AtomicU32 = AtomicU32::new(0);
#[cfg(not(any(test, feature = "test-support")))]
static RELOAD_LAST_NEXT_LOW: AtomicU32 = AtomicU32::new(0);
#[cfg(not(any(test, feature = "test-support")))]
static RELOAD_LAST_LAST: AtomicU32 = AtomicU32::new(u32::MAX);
#[cfg(not(any(test, feature = "test-support")))]
static RELOAD_LAST_HEAD: AtomicU32 = AtomicU32::new(u32::MAX);

#[cfg(not(any(test, feature = "test-support")))]
fn reset_reload_repair_evidence() {
    RELOAD_OBSERVATIONS.store(0, Ordering::Relaxed);
    RELOAD_UPPER_ONLY.store(0, Ordering::Relaxed);
    RELOAD_BASE_REPAIRS.store(0, Ordering::Relaxed);
    RELOAD_LAST_NEXT_LOW.store(0, Ordering::Relaxed);
    RELOAD_LAST_LAST.store(u32::MAX, Ordering::Relaxed);
    RELOAD_LAST_HEAD.store(u32::MAX, Ordering::Relaxed);
}

#[cfg(not(any(test, feature = "test-support")))]
fn increment_saturating(counter: &AtomicU32) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}

#[cfg(not(any(test, feature = "test-support")))]
const fn encode_optional_evidence(value: Option<u32>) -> u32 {
    match value {
        Some(value) => value,
        None => u32::MAX,
    }
}

#[cfg(not(any(test, feature = "test-support")))]
const fn decode_optional_evidence(value: u32) -> Option<u32> {
    if value == u32::MAX { None } else { Some(value) }
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
/// Taking it through [`RxDmaStorage::take_completed`](crate::rx_storage::RxDmaStorage::take_completed) records that this
/// descriptor must not be exposed a second time before its recycle group has
/// been rearmed. The token is deliberately neither `Copy` nor constructible
/// by callers: consuming it at the staging boundary represents the unique
/// software owner of that completed DMA descriptor.
#[derive(Debug, PartialEq, Eq)]
pub struct RxCompletedDescriptor {
    index: usize,
    descriptor_address: u32,
    word0: u32,
    next_descriptor_address: u32,
}

impl RxCompletedDescriptor {
    pub const fn index(&self) -> usize {
        self.index
    }

    pub const fn descriptor_address(&self) -> u32 {
        self.descriptor_address
    }

    pub const fn word0(&self) -> u32 {
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
/// Addresses can be reused immediately after append. Keeping the generation
/// and arena identity private prevents a stale LAST from authorizing mutation
/// of descriptors which have already returned to hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxFrozenCursor {
    last_descriptor_low: u32,
    next_descriptor_low: u32,
    descriptor_base: u32,
    arena_identity: usize,
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
    staged_word0: u32,
    total_length: usize,
    segment_lengths: RxCompletedUnitLengths,
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
    pub const fn staged_word0(&self) -> u32 {
        self.staged_word0
    }

    pub const fn total_length(&self) -> usize {
        self.total_length
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

/// Prepared zero-terminated RX ring while the hardware walker is stopped.
///
/// This type owns the right to start the descriptor walker. Consuming
/// [`try_start`](Self::try_start) transfers that authority into [`RxRingLive`].
pub struct RxRingStopped<'a, const COUNT: usize> {
    descriptors: &'a [Descriptor; COUNT],
    descriptor_base: u32,
    buffer_addresses: &'a RxDmaBufferAddresses<COUNT>,
    buffer_size: u16,
    initial_start: RingIndex<COUNT>,
    accepted_tail: RingIndex<COUNT>,
    retained_next_low: u32,
    retained_last_low: u32,
    binding: RxDmaBinding<'a>,
    lifecycle: Option<&'a AtomicU8>,
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
pub struct RxRingLive<'a, const COUNT: usize> {
    descriptors: &'a [Descriptor; COUNT],
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
    exhausted_republication_head_low: Option<u32>,
    #[cfg(any(test, feature = "test-support"))]
    reload_repair_evidence: RxReloadRepairEvidence,
    binding: RxDmaBinding<'a>,
    lifecycle: Option<&'a AtomicU8>,
    requires_stop: bool,
}

/// Descriptor storage authority after the hardware walker is confirmed off.
///
/// Live frontier bookkeeping is deliberately discarded at this lifecycle
/// edge. A later association must rebuild and republish the ring through
/// [`RxDmaStorage::prepare_halted`](crate::rx_storage::RxDmaStorage::prepare_halted), never resume descriptors from the previous
/// peer epoch.
pub struct RxRingHalted<'a, const COUNT: usize> {
    descriptors: &'a [Descriptor; COUNT],
    descriptor_base: u32,
    buffer_addresses: &'a RxDmaBufferAddresses<COUNT>,
    buffer_size: u16,
    lifecycle: Option<&'a AtomicU8>,
}

impl<'a, const COUNT: usize> RxRingHalted<'a, COUNT> {
    pub const fn descriptor_base(&self) -> u32 {
        self.descriptor_base
    }

    pub const fn descriptors(&self) -> &'a [Descriptor; COUNT] {
        self.descriptors
    }

    pub const fn buffer_addresses(&self) -> &RxDmaBufferAddresses<COUNT> {
        self.buffer_addresses
    }

    /// Observe one halted descriptor without recovering mutation authority.
    ///
    /// A halted ring can contain the terminal image of the preceding DMA
    /// epoch. This snapshot is useful for fault evidence before the next
    /// [`RxDmaStorage::prepare_halted`](crate::rx_storage::RxDmaStorage::prepare_halted) rebuilds every descriptor.
    pub fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        descriptor_snapshot(self.descriptors, self.descriptor_base, index)
    }

    /// Rebuild the stopped ring for a new ownership epoch.
    ///
    /// Failure returns the halted authority even if hardware observations or
    /// descriptor preparation rejected the attempt. The caller can then
    /// retry after a higher-level reset without stealing the static storage.
    pub(crate) fn prepare_owned<M, F>(
        self,
        mmio: &mut M,
        buffer_size: u32,
        prepare_buffer: F,
    ) -> Result<RxRingStopped<'a, COUNT>, (Self, RxRingError)>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        if u32::from(self.buffer_size) != buffer_size {
            return Err((self, RxRingError::Size));
        }
        let lifecycle = self.lifecycle;
        let buffer_addresses = self.buffer_addresses;
        match RxRingStopped::prepare_inner(
            mmio,
            self.descriptors,
            self.descriptor_base,
            buffer_addresses,
            buffer_size,
            prepare_buffer,
            self.lifecycle,
        ) {
            Ok(stopped) => Ok(stopped),
            Err(error) => {
                if mmio.walker_enabled()
                    && let Some(lifecycle) = lifecycle
                {
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

    /// Raw/model rebuild with caller-provided buffer preparation.
    ///
    /// Production code must use `RxDmaStorage::prepare_halted`, which binds
    /// every descriptor to the matching arena buffer and restores its guard
    /// words before publication.
    #[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
    pub fn prepare<M, F>(
        self,
        mmio: &mut M,
        buffer_size: u32,
        prepare_buffer: F,
    ) -> Result<RxRingStopped<'a, COUNT>, (Self, RxRingError)>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        self.prepare_owned(mmio, buffer_size, prepare_buffer)
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
            descriptors: self.descriptors,
            descriptor_base: self.descriptor_base,
            buffer_addresses: self.buffer_addresses,
            buffer_size: self.buffer_size,
            lifecycle: self.lifecycle,
        }
    }

    /// Stops the walker, prepares all buffers and publishes one physical-order
    /// cold list beginning at the retained hardware cursor, or descriptor zero
    /// when the preceding list was exhausted.
    ///
    /// `prepare_buffer` must restore any buffer-side DMA contract for `index`;
    /// for the S31 ROM layout this means the two `0xdead_beef` sentinels. It is
    /// invoked only while the walker is confirmed stopped.
    ///
    /// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS,ROM_REV0_HAL_MAC_RX_GATE,
    /// ROM_REV0_HAL_MAC_RX_LAST_DESCRIPTOR]. Live append/base repair is
    /// qualified by HIL_OPEN_RX_LIVE_APPEND_2026_07_27. A retained nonzero
    /// NEXT cursor is authoritative because WALKER_ENABLE does not clear it;
    /// the rebuilt zero-terminated list is rotated to that exact descriptor.
    #[cfg(any(test, feature = "test-support"))]
    pub fn prepare<M, F>(
        mmio: &mut M,
        descriptors: &'a [Descriptor; COUNT],
        descriptor_base: u32,
        buffer_addresses: &'a RxDmaBufferAddresses<COUNT>,
        buffer_size: u32,
        prepare_buffer: F,
    ) -> Result<Self, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        Self::prepare_inner(
            mmio,
            descriptors,
            descriptor_base,
            buffer_addresses,
            buffer_size,
            prepare_buffer,
            None,
        )
    }

    /// Build a ring from raw addresses for a target-only validation harness.
    ///
    /// # Safety
    ///
    /// Every descriptor and addressed buffer must remain allocated, aligned
    /// and exclusively DMA-owned until the returned ring is confirmed halted.
    /// Shipping code must use [`crate::rx_storage::RxDmaStorage::prepare_ring`]
    /// instead, which establishes that proof from a static arena.
    #[cfg(all(
        not(any(test, feature = "test-support")),
        feature = "validation-raw-dma"
    ))]
    #[allow(
        unsafe_code,
        reason = "raw target constructor makes its DMA lifetime contract explicit"
    )]
    pub unsafe fn prepare<M, F>(
        mmio: &mut M,
        descriptors: &'a [Descriptor; COUNT],
        descriptor_base: u32,
        buffer_addresses: &'a RxDmaBufferAddresses<COUNT>,
        buffer_size: u32,
        prepare_buffer: F,
    ) -> Result<Self, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        Self::prepare_inner(
            mmio,
            descriptors,
            descriptor_base,
            buffer_addresses,
            buffer_size,
            prepare_buffer,
            None,
        )
    }

    pub(crate) fn prepare_inner<M, F>(
        mmio: &mut M,
        descriptors: &'a [Descriptor; COUNT],
        descriptor_base: u32,
        buffer_addresses: &'a RxDmaBufferAddresses<COUNT>,
        buffer_size: u32,
        mut prepare_buffer: F,
        lifecycle: Option<&'a AtomicU8>,
    ) -> Result<Self, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        validate_live_ring_geometry::<COUNT>()?;
        let buffer_size_u16 = u16::try_from(buffer_size).map_err(|_| RxRingError::Size)?;
        if rx_armed_word(buffer_size).is_none() {
            return Err(RxRingError::Size);
        }
        let binding = RxDmaBinding::new(descriptors, descriptor_base).ok_or(RxRingError::Count)?;
        if mmio.walker_enabled() {
            disable_receive_inner(mmio)?;
        }
        // NEXT and LAST belong to the walker. They are stable handoff
        // frontiers only after WALKER_ENABLE has been confirmed clear.
        mmio.fence();
        let retained_next_low = mmio.next_descriptor_low();
        let retained_last_low = mmio.last_descriptor_low();
        // Disabling WALKER_ENABLE does not reset RX_NEXT_DESCRIPTOR. A later
        // enable resumes that cursor when it is nonzero, even if software has
        // written a different BASE. Rebuild the zero-terminated list from the
        // retained cursor so hardware and the Rust recycle frontier name the
        // same first descriptor. An exhausted cursor is zero; in that case a
        // fresh BASE publication legitimately starts from physical slot zero.
        let initial_start = if retained_next_low == 0 {
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
        publish_cold_ring_inner(mmio, &binding, head, false)?;

        Ok(Self {
            descriptors,
            descriptor_base,
            buffer_addresses,
            buffer_size: buffer_size_u16,
            initial_start,
            accepted_tail: initial_start.previous(),
            retained_next_low,
            retained_last_low,
            binding,
            lifecycle,
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

    /// Opens the walker and consumes the stopped-state authority.
    ///
    /// The caller owns any platform-specific settle delay between
    /// ring preparation and this edge.
    pub fn try_start<M: RxDma>(
        self,
        mmio: &mut M,
    ) -> Result<RxRingLive<'a, COUNT>, (Self, RxRingError)> {
        if let Some(lifecycle) = self.lifecycle {
            match RxDmaArenaState::load(lifecycle) {
                RxDmaArenaState::Prepared => {}
                RxDmaArenaState::ResetRequired => {
                    return Err((self, RxRingError::ResetRequired));
                }
                RxDmaArenaState::Reusable | RxDmaArenaState::Live => {
                    return Err((self, RxRingError::Busy));
                }
            }
        }
        if !self.topology_snapshot().valid {
            return Err((self, RxRingError::Corrupt));
        }
        if let Err(error) = enable_receive_inner(mmio, &self.binding) {
            if mmio.walker_enabled()
                && let Some(lifecycle) = self.lifecycle
            {
                // A failed start which nevertheless observes WALKER_ENABLE
                // cannot return an honestly stopped capability. Keep the
                // static backing quarantined; the returned value exists only
                // to preserve diagnostics/ownership until radio reset.
                RxDmaArenaState::ResetRequired.store(lifecycle);
            }
            return Err((self, error));
        }
        if let Some(lifecycle) = self.lifecycle {
            RxDmaArenaState::Live.store(lifecycle);
        }
        #[cfg(not(any(test, feature = "test-support")))]
        reset_reload_repair_evidence();
        Ok(RxRingLive {
            descriptors: self.descriptors,
            descriptor_base: self.descriptor_base,
            buffer_addresses: self.buffer_addresses,
            buffer_size: self.buffer_size,
            observed_mask: RxObservedMask::default(),
            recycle_start: self.initial_start,
            accepted_tail: self.accepted_tail,
            pending_tail_plus_one: 0,
            cursor_generation: 0,
            completion_release_probe_pending: false,
            exhausted_republication_head_low: None,
            #[cfg(any(test, feature = "test-support"))]
            reload_repair_evidence: RxReloadRepairEvidence::default(),
            binding: self.binding,
            lifecycle: self.lifecycle,
            requires_stop: true,
        })
    }
}

impl<'a, const COUNT: usize> RxRingLive<'a, COUNT> {
    pub const fn descriptor_base(&self) -> u32 {
        self.descriptor_base
    }

    pub(crate) const fn descriptors(&self) -> &'a [Descriptor; COUNT] {
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
    /// live epoch. Raw validation rings carry no lifecycle and deliberately
    /// fall back to complete address validation.
    pub(crate) fn belongs_to_lifecycle(&self, lifecycle: &AtomicU8) -> bool {
        self.lifecycle
            .is_some_and(|bound| core::ptr::eq(bound, lifecycle))
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

    pub fn reload_repair_evidence(&self) -> RxReloadRepairEvidence {
        #[cfg(not(any(test, feature = "test-support")))]
        {
            RxReloadRepairEvidence {
                observations: RELOAD_OBSERVATIONS.load(Ordering::Relaxed),
                unknown_upper_with_zero_address: RELOAD_UPPER_ONLY.load(Ordering::Relaxed),
                base_repairs: RELOAD_BASE_REPAIRS.load(Ordering::Relaxed),
                last_next_low: RELOAD_LAST_NEXT_LOW.load(Ordering::Relaxed),
                last_last_low: decode_optional_evidence(RELOAD_LAST_LAST.load(Ordering::Relaxed)),
                last_repair_head: decode_optional_evidence(
                    RELOAD_LAST_HEAD.load(Ordering::Relaxed),
                ),
            }
        }
        #[cfg(any(test, feature = "test-support"))]
        {
            self.reload_repair_evidence
        }
    }

    fn record_reload_repair_observation(
        &mut self,
        next_descriptor: crate::rx_dma::RxDmaNextDescriptor,
        last_descriptor_low: Option<u32>,
        repair_head: Option<u32>,
    ) {
        let upper_only =
            next_descriptor.address_low() == 0 && next_descriptor.has_unknown_upper_state();
        #[cfg(not(any(test, feature = "test-support")))]
        {
            increment_saturating(&RELOAD_OBSERVATIONS);
            if upper_only {
                increment_saturating(&RELOAD_UPPER_ONLY);
            }
            RELOAD_LAST_NEXT_LOW.store(next_descriptor.address_low(), Ordering::Relaxed);
            RELOAD_LAST_LAST.store(
                encode_optional_evidence(last_descriptor_low),
                Ordering::Relaxed,
            );
            if let Some(repair_head) = repair_head {
                increment_saturating(&RELOAD_BASE_REPAIRS);
                RELOAD_LAST_HEAD.store(repair_head, Ordering::Relaxed);
            }
        }
        #[cfg(any(test, feature = "test-support"))]
        {
            self.reload_repair_evidence.observations =
                self.reload_repair_evidence.observations.saturating_add(1);
            if upper_only {
                self.reload_repair_evidence.unknown_upper_with_zero_address = self
                    .reload_repair_evidence
                    .unknown_upper_with_zero_address
                    .saturating_add(1);
            }
            self.reload_repair_evidence.last_next_low = next_descriptor.address_low();
            self.reload_repair_evidence.last_last_low = last_descriptor_low;
            if let Some(repair_head) = repair_head {
                self.reload_repair_evidence.base_repairs =
                    self.reload_repair_evidence.base_repairs.saturating_add(1);
                self.reload_repair_evidence.last_repair_head = Some(repair_head);
            }
        }
    }

    /// Irreversibly poison the static arena after an ownership invariant can
    /// no longer be proven. The live token may still be retained for reset
    /// diagnostics, but no later stop observation can make this arena reusable.
    pub fn require_reset(&mut self) {
        if let Some(lifecycle) = self.lifecycle {
            RxDmaArenaState::ResetRequired.store(lifecycle);
        }
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
        if let Err(error) = disable_receive_inner(mmio) {
            return Err((self, error));
        }
        let halted = RxRingHalted {
            descriptors: self.descriptors,
            descriptor_base: self.descriptor_base,
            buffer_addresses: self.buffer_addresses,
            buffer_size: self.buffer_size,
            lifecycle: self.lifecycle,
        };
        if let Some(lifecycle) = self.lifecycle
            && RxDmaArenaState::load(lifecycle) != RxDmaArenaState::ResetRequired
        {
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
                || !rx_done(self.descriptors[index.get()].word0())
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
                || !rx_done(self.descriptors[index.get()].word0())
            {
                break;
            }
            completed += 1;
        }
        completed
    }

    /// Snapshot complete RX units, including units split across descriptors.
    ///
    /// A non-terminal descriptor is never reported by itself: the scan only
    /// extends `descriptor_count` after observing a later `RX_DONE` terminal.
    /// Thus an armed or partially filled frontier remains owned by hardware.
    pub fn completed_unit_frontier(&self) -> RxCompletedUnitFrontier {
        self.completed_unit_frontier_with_limit(COUNT, |_| true)
    }

    /// Snapshot a complete RX unit only through the descriptor frontier which
    /// hardware has published in `RX_LAST_DESCRIPTOR`.
    ///
    /// SOURCE: the complete vendor walker
    /// `libpp[wdev.o]::wdevProcessRxSucDataAll` snapshots `libpp[hal_mac_rx.o]::hal_mac_rx_get_last_dscr` before touching the software list,
    /// processes through that descriptor, then refreshes the snapshot before
    /// extending the same pass. `RX_DONE` alone is therefore not sufficient
    /// ownership evidence: its store may be observed before the MAC has
    /// finished advancing its descriptor frontier. Returning such a node to
    /// the live append list can overwrite `next` while hardware still needs
    /// it, terminating RX after the first frame.
    pub fn completed_unit_frontier_through_with<F>(
        &self,
        last_descriptor_low: u32,
        nonterminal_consumed: F,
    ) -> RxCompletedUnitFrontier
    where
        F: FnMut(usize) -> bool,
    {
        let Some(last_index) = descriptor_index::<COUNT>(last_descriptor_low, self.descriptor_base)
        else {
            return RxCompletedUnitFrontier::default();
        };
        let observed = self.observed_prefix_len();
        if observed == COUNT {
            return RxCompletedUnitFrontier::default();
        }
        let last_distance = self.recycle_start.distance_to(last_index);
        let accepted_distance = self.recycle_start.distance_to(self.accepted_tail);
        if last_distance < observed || last_distance > accepted_distance {
            return RxCompletedUnitFrontier::default();
        }
        let descriptor_limit = last_distance + 1 - observed;
        self.completed_unit_frontier_with_limit(descriptor_limit, nonterminal_consumed)
    }

    /// Snapshot complete units through one ordered LAST/NEXT cursor image.
    ///
    /// A zero NEXT with LAST at the accepted tail proves that the finite
    /// hardware list was exhausted. In that state every link from the
    /// software head through LAST has been consumed, even when an unused
    /// intermediate buffer retained its recycle guard. The complete vendor
    /// `wdevProcessRxSucDataAll` likewise walks every node through its saved
    /// LAST and uses only terminal bit 30 to delimit receive units.
    pub fn completed_unit_frontier_through_cursor_with<F>(
        &self,
        last_descriptor_low: u32,
        next_descriptor_low: u32,
        nonterminal_consumed: F,
    ) -> RxCompletedUnitFrontier
    where
        F: FnMut(usize) -> bool,
    {
        if next_descriptor_low == 0
            && descriptor_index::<COUNT>(last_descriptor_low, self.descriptor_base)
                == Some(self.accepted_tail)
        {
            self.completed_unit_frontier_through_with(last_descriptor_low, |_| true)
        } else {
            self.completed_unit_frontier_through_with(last_descriptor_low, nonterminal_consumed)
        }
    }

    /// Snapshot only the first complete unit through the hardware LAST
    /// frontier. Unlike [`Self::completed_unit_frontier_through_with`], a run
    /// of single-descriptor frames is deliberately truncated to one unit so
    /// callers can prove and recycle each link before taking the next owner.
    pub fn first_completed_unit_frontier_through_with<F>(
        &self,
        last_descriptor_low: u32,
        nonterminal_consumed: F,
    ) -> RxCompletedUnitFrontier
    where
        F: FnMut(usize) -> bool,
    {
        let Some(last_index) = descriptor_index::<COUNT>(last_descriptor_low, self.descriptor_base)
        else {
            return RxCompletedUnitFrontier::default();
        };
        let observed = self.observed_prefix_len();
        if observed == COUNT {
            return RxCompletedUnitFrontier::default();
        }
        let last_distance = self.recycle_start.distance_to(last_index);
        let accepted_distance = self.recycle_start.distance_to(self.accepted_tail);
        if last_distance < observed || last_distance > accepted_distance {
            return RxCompletedUnitFrontier::default();
        }
        let descriptor_limit = last_distance + 1 - observed;
        self.first_completed_unit_frontier_with_limit(descriptor_limit, nonterminal_consumed)
    }

    /// Unit-sized counterpart of
    /// [`Self::completed_unit_frontier_through_cursor_with`].
    pub fn first_completed_unit_frontier_through_cursor_with<F>(
        &self,
        last_descriptor_low: u32,
        next_descriptor_low: u32,
        nonterminal_consumed: F,
    ) -> RxCompletedUnitFrontier
    where
        F: FnMut(usize) -> bool,
    {
        if next_descriptor_low == 0
            && descriptor_index::<COUNT>(last_descriptor_low, self.descriptor_base)
                == Some(self.accepted_tail)
        {
            self.first_completed_unit_frontier_through_with(last_descriptor_low, |_| true)
        } else {
            self.first_completed_unit_frontier_through_with(
                last_descriptor_low,
                nonterminal_consumed,
            )
        }
    }

    /// Variant of [`completed_unit_frontier`](Self::completed_unit_frontier)
    /// that requires buffer-side evidence for each non-terminal descriptor.
    ///
    /// An armed descriptor and a consumed full non-terminal descriptor can
    /// have the same ownership word. The storage owner can disambiguate them
    /// by checking the buffer guard restored at recycle. This also prevents a
    /// terminal left elsewhere in a handoff epoch from being joined to an
    /// untouched frontier descriptor.
    pub fn completed_unit_frontier_with<F>(
        &self,
        nonterminal_consumed: F,
    ) -> RxCompletedUnitFrontier
    where
        F: FnMut(usize) -> bool,
    {
        self.completed_unit_frontier_with_limit(COUNT, nonterminal_consumed)
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
        if rx_done(self.descriptors[first_index.get()].word0()) {
            let mut completed = 0;
            while completed < descriptor_limit {
                let index = first_index.advance(completed);
                if self.observed_mask.contains(index.get())
                    || !rx_done(self.descriptors[index.get()].word0())
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
            if rx_done(self.descriptors[index.get()].word0()) {
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
        if rx_done(self.descriptors[first_index.get()].word0()) {
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
            if rx_done(self.descriptors[index.get()].word0()) {
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
    /// `descriptor_limit` should come from a prior
    /// [`completed_unit_frontier`](Self::completed_unit_frontier) snapshot. It
    /// prevents a saturated producer from extending this ownership transfer
    /// to a later completion epoch.
    pub(crate) fn take_completed_unit_owned<F>(
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
        let first_length = descriptor_length(first_word0);
        if rx_done(first_word0) {
            let encoded_length = first_length;
            let descriptor_address = descriptor_address(self.descriptor_base, first_index.get())?;
            let segment_length = u16::try_from(first_length).map_err(|_| RxRingError::Size)?;
            self.observed_mask.insert(first_index.get());
            return Ok(Some(RxCompletedUnit {
                head_index: first_index.get(),
                descriptor_count: 1,
                descriptor_address,
                staged_word0: (first_word0 & !(SIZE_MASK | LENGTH_MASK))
                    | encoded_length
                    | (encoded_length << LENGTH_SHIFT)
                    | BIT_30
                    | BIT_31,
                total_length: first_length as usize,
                segment_lengths: RxCompletedUnitLengths::Single(segment_length),
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
            let consumed = rx_done(word0) || nonterminal_consumed(index.get());
            let length = if consumed {
                descriptor_length(word0)
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
            if !rx_done(word0) {
                continue;
            }
            if total_length > SIZE_MASK as usize {
                return Err(RxRingError::Size);
            }
            let descriptor_count = step + 1;
            let encoded_length = u32::try_from(total_length).map_err(|_| RxRingError::Overflow)?;
            let descriptor_address =
                descriptor_address(self.descriptor_base, metadata_index.get())?;
            self.observed_mask
                .insert_group::<COUNT>(first_index, descriptor_count);
            return Ok(Some(RxCompletedUnit {
                head_index: first_index.get(),
                descriptor_count,
                descriptor_address,
                staged_word0: (metadata_word0 & !(SIZE_MASK | LENGTH_MASK))
                    | encoded_length
                    | (encoded_length << LENGTH_SHIFT)
                    | BIT_30
                    | BIT_31,
                total_length,
                segment_lengths: RxCompletedUnitLengths::Chained(segment_lengths),
            }));
        }
        Ok(None)
    }

    /// Raw/model completion transfer without a storage-bound buffer owner.
    #[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
    pub fn take_completed_unit(
        &mut self,
        descriptor_limit: usize,
    ) -> Result<Option<RxCompletedUnit>, RxRingError> {
        self.take_completed_unit_owned(descriptor_limit, |_| true)
    }

    crate::place_rx_hot_path! {
    /// Takes one newly completed descriptor exactly once for this ring epoch.
    ///
    /// Kept in internal SRAM for PSRAM-code profiles: this is invoked once for
    /// every descriptor slot on every receive poll. Hardware runs at HE20
    /// showed that executing the complete poll/copy path from PSRAM capped
    /// useful UDP RX near 65 Mbit/s.
    #[inline(never)]
    pub(crate) fn take_completed_owned(&mut self, index: usize) -> Option<RxCompletedDescriptor> {
        if index >= COUNT {
            return None;
        }
        if self.observed_mask.contains(index) {
            return None;
        }
        let descriptor = &self.descriptors[index];
        let word0 = descriptor.word0();
        if !rx_done(word0) {
            return None;
        }
        self.validate_descriptor_image(RingIndex::checked(index)?).ok()?;
        let descriptor_address = descriptor_address(self.descriptor_base, index).ok()?;
        self.observed_mask.insert(index);
        Some(RxCompletedDescriptor {
            index,
            descriptor_address,
            word0,
            next_descriptor_address: descriptor.next_address(),
        })
    }}

    /// Raw/model descriptor transfer without a storage-bound buffer owner.
    #[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
    pub fn take_completed(&mut self, index: usize) -> Option<RxCompletedDescriptor> {
        self.take_completed_owned(index)
    }

    pub(crate) fn validate_completed_descriptor(&self, index: usize) -> Result<bool, RxRingError> {
        let descriptor = self.descriptors.get(index).ok_or(RxRingError::Count)?;
        let word0 = descriptor.word0();
        if !rx_done(word0) {
            return Ok(false);
        }
        self.validate_descriptor_image(RingIndex::checked(index).ok_or(RxRingError::Count)?)?;
        Ok(true)
    }

    fn validate_descriptor_image(&self, index: RingIndex<COUNT>) -> Result<u32, RxRingError> {
        let descriptor = self
            .descriptors
            .get(index.get())
            .ok_or(RxRingError::Count)?;
        let word0 = descriptor.word0();
        let expected_size = u32::from(self.buffer_size);
        if descriptor_size(word0) != expected_size
            || descriptor_length(word0) > expected_size
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
    /// during reload, the exact ROM base-repair rule is applied before the new
    /// tail becomes accepted.
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

    /// Raw/model half-ring recycle with caller-provided buffer preparation.
    #[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
    pub fn recycle_completed_half<M, F>(
        &mut self,
        mmio: &mut M,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        self.recycle_completed_half_owned(mmio, prepare_buffer)
    }

    crate::place_rx_hot_path! {
    /// Rearm and append one fixed-size, contiguous group of completed slots.
    ///
    /// The complete ROM `wDev_AppendRxBlocks` contract accepts an arbitrary
    /// zero-terminated `head..tail` chain; a half-ring is not a hardware
    /// requirement. Smaller groups let a task replenish the walker before a
    /// peer's next A-MPDU consumes every remaining descriptor. `BATCH` must
    /// divide the ring so `recycle_start` visits each slot exactly once.
    ///
    /// The caller must still drive [`Self::poll_pending_reload`] after each
    /// successful append. This deliberately preserves the ROM doorbell/base
    /// repair ordering without hiding a wait in this finite operation.
    #[inline(never)]
    #[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
    pub fn recycle_completed_batch<const BATCH: usize, M, F>(
        &mut self,
        mmio: &mut M,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        if BATCH == 0 || BATCH > COUNT || !COUNT.is_multiple_of(BATCH) {
            return Err(RxRingError::Count);
        }
        self.recycle_completed_group(
            mmio,
            BATCH,
            RxRecycleGroupShape::EveryDescriptorTerminal,
            prepare_buffer,
        )
    }}

    crate::place_rx_hot_path! {
    /// Rearm the longest currently completed prefix, up to `MAX_BATCH`.
    ///
    /// Unlike the fixed-size raw/model batch operation, this does not wait for a
    /// fixed-size group. It is the closer ownership model for the complete
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

        let mut completed = 0;
        while completed < MAX_BATCH {
            let index = self.recycle_start.advance(completed);
            if !self.observed_mask.contains(index.get()) || !rx_done(self.descriptors[index.get()].word0()) {
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

    /// Raw/model prefix recycle with caller-provided buffer preparation.
    #[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
    pub fn recycle_completed_prefix<const MAX_BATCH: usize, M, F>(
        &mut self,
        mmio: &mut M,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        self.recycle_completed_prefix_owned::<MAX_BATCH, _, _>(mmio, prepare_buffer)
    }

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
    /// Binding the operation to [`RxFrozenCursor`] prevents the descriptor
    /// address from being reused as authority after this append changes the
    /// ring generation.
    pub(crate) fn recycle_completed_unit_through_frozen_last_owned<M, F>(
        &mut self,
        mmio: &mut M,
        cursor: RxFrozenCursor,
        descriptor_count: usize,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        if !self.frozen_cursor_releases_unit(cursor, descriptor_count) {
            return Ok(None);
        }
        self.recycle_completed_group(
            mmio,
            descriptor_count,
            RxRecycleGroupShape::OneCompletedUnitThroughFrozenLast,
            prepare_buffer,
        )
    }

    /// Raw/model unit recycle with caller-provided buffer preparation.
    #[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
    pub fn recycle_completed_unit<M, F>(
        &mut self,
        mmio: &mut M,
        descriptor_count: usize,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        self.recycle_completed_unit_owned(mmio, descriptor_count, prepare_buffer)
    }

    fn recycle_completed_group<M, F>(
        &mut self,
        mmio: &mut M,
        group_size: usize,
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

        if !self
            .observed_mask
            .contains_group::<COUNT>(self.recycle_start, group_size)
        {
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
                RxRecycleGroupShape::EveryDescriptorTerminal => rx_done(word0),
                RxRecycleGroupShape::OneCompletedUnit
                | RxRecycleGroupShape::OneCompletedUnitThroughFrozenLast => {
                    rx_done(word0) == (step + 1 == group_size)
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
        let accepted_distance = self.recycle_start.distance_to(self.accepted_tail);
        if group_size - 1 > accepted_distance {
            // A completed-looking descriptor beyond the accepted hardware
            // tail is not part of the live list. Reject the complete
            // transaction before modifying buffer guards or descriptor words.
            self.require_reset();
            return Err(RxRingError::Corrupt);
        }
        let append_to_empty = tail_index == self.accepted_tail;
        if !append_to_empty && self.descriptors[self.accepted_tail.get()].next_address() != 0 {
            // The accepted tail is the only legal append point. Validate it
            // before the first irreversible buffer/descriptor mutation so a
            // corrupt list cannot become a partially rearmed live arena.
            self.require_reset();
            return Err(RxRingError::Corrupt);
        }
        // The frozen-LAST path validates its generation-specific cursor proof
        // before entering this mutation routine. Raw/model paths still need a
        // fresh ownership observation here.
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
            descriptor.publish_owned(
                rx_rearm_word(descriptor.word0()).ok_or(RxRingError::Size)?,
                buffer_address,
                next,
            );
        }

        // `wDev_DiscardFrame` advances the software head before calling
        // `wDev_AppendRxBlocks`. If the discarded unit was also the accepted
        // tail, that advance makes the old list empty. The vendor append path
        // then publishes the returned chain directly through RX_DESCRIPTOR_BASE;
        // it must not link the descriptor to itself and ring the reload
        // doorbell. `accepted_tail` is our tail authority, so membership of
        // that exact terminal node in this prefix is the equivalent proof.
        if append_to_empty {
            mmio.fence();
            self.publish_exhausted_descriptor_base(mmio, head_address);
            self.cursor_generation = next_cursor_generation;
            self.accepted_tail = tail_index;
            self.pending_tail_plus_one = 0;
            // A terminal-only list can be filled and exhausted without a
            // second usable RX-success edge. The vendor worker continues to
            // inspect its durable software list after this direct BASE path.
            // Preserve that behavior explicitly instead of relying on an IRQ
            // from a walker which may already have reached NEXT=0 again.
            self.observed_mask
                .remove_group::<COUNT>(self.recycle_start, group_size);
            // The software list was empty before this append. Its new head is
            // therefore the returned chain's head, not the physical slot
            // following its tail. `wDev_DiscardFrame` first stores the old
            // `head->next` (null here) into `wDevCtrl.head`, after which the
            // empty branch of `wDev_AppendRxBlocks` stores its argument head
            // back into `wDevCtrl.head` and RX_DESCRIPTOR_BASE. Advancing to
            // the following slot would make software skip the very list that
            // hardware has just started consuming.
            self.recycle_start = head_index;
            return Ok(Some(RxLiveAppend {
                head_index,
                head_address,
                tail_index,
                descriptor_count: group_size,
            }));
        }
        let accepted_tail = &self.descriptors[self.accepted_tail.get()];
        // This type is the sole publication authority. All descriptors in the
        // appended group were observed complete, rearmed and remain
        // unreachable until this old-tail link and the following doorbell.
        accepted_tail.publish_next_address(head_address);
        mmio.fence();
        mmio.request_reload(&self.binding);
        mmio.fence();

        self.cursor_generation = next_cursor_generation;
        self.pending_tail_plus_one = tail_index.get() + 1;
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
        next: crate::rx_dma::RxDmaNextDescriptor,
    ) -> Option<usize> {
        if next.is_zero() {
            return Some(0);
        }
        let next_index = descriptor_index::<COUNT>(next.address_low(), self.descriptor_base)?;
        Some(next_index.distance_to(self.accepted_tail) + 1)
    }

    /// Freeze one ordered LAST/NEXT image for this descriptor generation.
    pub fn freeze_cursor<M: RxDma>(&self, mmio: &mut M) -> RxFrozenCursor {
        let (last_descriptor_low, next_descriptor_low) = mmio.with_ordered_cursor(|cursor| {
            (cursor.last_descriptor_low(), cursor.next_descriptor_low())
        });
        // Make both MMIO observations precede descriptor and payload reads by
        // the caller which receives this generation-bound snapshot.
        mmio.fence();
        RxFrozenCursor {
            last_descriptor_low,
            next_descriptor_low,
            descriptor_base: self.descriptor_base,
            arena_identity: self.descriptors.as_ptr() as usize,
            generation: self.cursor_generation,
        }
    }

    /// Return whether an exhausted finite hardware list still owes software
    /// the accepted terminal descriptor's writeback.
    ///
    /// Descriptor writeback can become visible after this cursor edge and no
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

    /// Raw/model release proof from an explicitly supplied LAST snapshot.
    #[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
    pub fn observe_completed_unit_link_release<M: RxDma>(
        &mut self,
        mmio: &mut M,
        last_descriptor_low: u32,
        descriptor_count: usize,
    ) -> bool {
        let next_descriptor_low = mmio.next_descriptor_low();
        mmio.fence();
        self.observe_completed_unit_link_release_from_snapshot(
            last_descriptor_low,
            next_descriptor_low,
            descriptor_count,
        )
    }

    /// Whether a frozen vendor-style LAST snapshot releases the current unit.
    ///
    /// `wdevProcessRxSucDataAll` mutates and returns the descriptor equal to
    /// its saved LAST before taking another snapshot. Keep this proof separate
    /// from the fallback live-cursor probe: it is valid only while the caller
    /// has not appended anything since freezing `last_descriptor_low`.
    fn frozen_cursor_releases_unit(&self, cursor: RxFrozenCursor, descriptor_count: usize) -> bool {
        if descriptor_count == 0 || descriptor_count > COUNT {
            return false;
        }
        if cursor.descriptor_base != self.descriptor_base
            || cursor.arena_identity != self.descriptors.as_ptr() as usize
            || cursor.generation != self.cursor_generation
        {
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
                rx_done(self.descriptors[self.recycle_start.advance(distance).get()].word0())
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

    /// Whether direct BASE publication of a previously empty software list
    /// requires one durable follow-up ownership probe.
    pub const fn exhausted_republication_probe_pending(&self) -> bool {
        self.exhausted_republication_head_low.is_some()
    }

    /// Publish a descriptor head after the walker reached a zero successor.
    ///
    /// A BASE store is not an interrupt-producing ownership edge. The newly
    /// published list can be consumed through its terminal descriptor before
    /// software receives another usable RX-success interrupt, so every live
    /// direct BASE publication must retain a cooperative follow-up probe.
    fn publish_exhausted_descriptor_base<M: RxDma>(&mut self, mmio: &mut M, head_address: u32) {
        mmio.write_descriptor_base(&self.binding, head_address);
        mmio.fence();
        self.exhausted_republication_head_low = Some(head_address & RX_DESCRIPTOR_ADDRESS_LOW_MASK);
    }

    /// Observe whether the walker has accepted a directly republished head.
    ///
    /// Time is not ownership evidence. Keep the cooperative probe armed while
    /// NEXT remains zero: the BASE store may not yet have reached the walker,
    /// and an exhausted walker cannot provide a later interrupt edge. Any
    /// matching NEXT proves that hardware has fetched the published head, but
    /// it does not immediately retire the probe. One further cooperative
    /// observation closes the race where that head completes while the task
    /// is still consuming the IRQ which exhausted the previous list. After
    /// that yield, either the completion is visible to the ordinary
    /// LAST-bounded frontier or a later completion produces a fresh IRQ. A
    /// different nonzero cursor may be the retained image of the exhausted
    /// list and is not an ownership edge.
    pub fn observe_exhausted_republication<M: RxDma>(&mut self, mmio: &mut M) {
        let Some(probe) = self.exhausted_republication_head_low else {
            return;
        };
        if probe & RX_EXHAUSTED_REPUBLICATION_ACCEPTED != 0 {
            self.exhausted_republication_head_low = None;
        } else if mmio.next_descriptor_low() == probe {
            self.exhausted_republication_head_low =
                Some(probe | RX_EXHAUSTED_REPUBLICATION_ACCEPTED);
        }
    }

    /// Observe one live-append doorbell edge without waiting.
    ///
    /// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS]: the complete ROM body at
    /// `0x2f83_8a7e` spins on `RX_CONTROL.APPEND_DESCRIPTOR_RELOAD` with the
    /// exact `0x186a1` bound, then immediately samples `RX_NEXT_DESCRIPTOR`
    /// and repairs `RX_DESCRIPTOR_BASE` from `last->next` when the old
    /// frontier was exhausted. Deferring this suffix until after another RX
    /// processing pass is observably unsafe: the appended half can itself
    /// reach the terminal descriptor first, making `RX_LAST_DESCRIPTOR`
    /// indistinguishable from the no-repair case.
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
        Ok(RxReloadObservation::Settled)
    }

    /// Complete the reload doorbell and its conditional BASE-repair suffix as
    /// one unscheduled transaction.
    ///
    /// `SOURCE[ROM_REV0_WDEV_APPEND_RX_BLOCKS]`: the vendor function performs
    /// the bounded `RX_CONTROL` loop and immediately samples `NEXT`, then
    /// conditional `LAST`, without returning to its scheduler. Yielding after
    /// a pending sample lets a later RX completion replace that cursor epoch
    /// and can authorize a stale BASE repair for the descriptor just recycled.
    pub fn complete_pending_reload<M: RxDma>(&mut self, mmio: &mut M) -> Result<(), RxRingError> {
        if self.pending_tail().is_none() {
            return Ok(());
        }
        for _ in 0..RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT {
            if mmio.try_with_reload_settled(|_settled| ()).is_some() {
                return self.settle_reload(mmio);
            }
        }
        Err(RxRingError::Busy)
    }

    fn settle_reload<M: RxDma>(&mut self, mmio: &mut M) -> Result<(), RxRingError> {
        let Some(pending_tail) = self.pending_tail() else {
            return Ok(());
        };
        let mut next_descriptor = crate::rx_dma::RxDmaNextDescriptor::default();
        let mut last_descriptor_low = None;
        let repair_head = mmio.with_reload_repair_observation(|observation| {
            next_descriptor = observation.next_descriptor();
            last_descriptor_low = observation.exhausted_last_descriptor_low();
            if !next_descriptor.is_zero() {
                return Ok(None);
            }
            let last_descriptor_low = last_descriptor_low.ok_or(RxRingError::Corrupt)?;
            let last_index = descriptor_index::<COUNT>(last_descriptor_low, self.descriptor_base)
                .ok_or(RxRingError::Corrupt)?;
            if last_index == pending_tail {
                return Ok(None);
            }
            {
                // SOURCE: `libpp[wdev.o]::wDev_AppendRxBlocks`: after reload clears,
                // `NEXT=0` and `LAST != new_tail` lead directly to
                // `BASE = LAST->next`. LAST may still name an earlier node of
                // the accepted list when several completed units are detached
                // and appended in one worker turn; it is not restricted to
                // the immediately preceding software tail. Keep only the
                // vendor's actual safety conditions: LAST and its nonzero
                // successor must both belong to this descriptor arena.
                let repair_head = self.descriptors[last_index.get()].next_address();
                if repair_head == 0
                    || descriptor_index_full::<COUNT>(repair_head, self.descriptor_base).is_none()
                {
                    return Err(RxRingError::Corrupt);
                }
                Ok(Some(repair_head))
            }
        });
        let repair_head = match repair_head {
            Ok(repair_head) => repair_head,
            Err(error) => {
                self.record_reload_repair_observation(next_descriptor, last_descriptor_low, None);
                self.require_reset();
                return Err(error);
            }
        };
        self.record_reload_repair_observation(next_descriptor, last_descriptor_low, repair_head);
        if let Some(repair_head) = repair_head {
            // The reload suffix is a second direct BASE publication path, not
            // merely completion of the doorbell transaction. Preserve a
            // durable service continuation exactly as for publication of a
            // previously empty software list; an exhausted walker cannot be
            // relied on to produce another IRQ after this store.
            self.publish_exhausted_descriptor_base(mmio, repair_head);
        }
        self.accepted_tail = pending_tail;
        self.pending_tail_plus_one = 0;
        Ok(())
    }
}

impl<const COUNT: usize> Drop for RxRingLive<'_, COUNT> {
    fn drop(&mut self) {
        if self.requires_stop
            && let Some(lifecycle) = self.lifecycle
        {
            RxDmaArenaState::ResetRequired.store(lifecycle);
        }
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
    descriptors: &[Descriptor; COUNT],
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
    descriptors: &[Descriptor; COUNT],
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
    if validate_live_ring_geometry::<COUNT>().is_err() || !descriptor_address_valid(descriptor_base)
    {
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
            let capacity = descriptor_size(word0);
            if capacity == 0
                || !dma_range_valid(buffer_address, capacity)
                || descriptor_length(word0) != capacity
                || word0 & BIT_31 == 0
                || word0 & (BIT_29 | BIT_30) != 0
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
    descriptors: &[Descriptor; COUNT],
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
        descriptors[index.get()].publish_owned(word0, buffer_addresses[index.get()], next);
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
    descriptors: &[Descriptor],
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
    if descriptor_dma_base & 3 != 0 || !dma_range_valid(descriptor_dma_base, span) {
        return Err(RxRingError::Address);
    }
    for &buffer in buffer_addresses {
        if !dma_range_valid(buffer, buffer_size) {
            return Err(RxRingError::Address);
        }
    }
    for (index, descriptor) in descriptors.iter().enumerate() {
        let next = if index + 1 < descriptors.len() {
            descriptor_dma_base + (index as u32 + 1) * DESCRIPTOR_BYTES
        } else {
            0
        };
        descriptor.publish_owned(word0, buffer_addresses[index], next);
    }
    Ok(())
}

/// Build a synthetic cold descriptor list for native models or an explicit
/// raw-DMA validation probe.
///
/// Ordinary target code must construct a ring through [`RxRingStopped`],
/// which couples mutation to the arena lifecycle instead of accepting a bare
/// descriptor slice.
#[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
pub fn build_cold_ring(
    descriptors: &[Descriptor],
    descriptor_dma_base: u32,
    buffer_addresses: &[u32],
    buffer_size: u32,
) -> Result<(), RxRingError> {
    build_cold_ring_inner(
        descriptors,
        descriptor_dma_base,
        buffer_addresses,
        buffer_size,
    )
}

/// Publish a synthetic cold ring in a native model with no DMA actor.
#[cfg(any(test, feature = "test-support"))]
pub fn publish_cold_ring<M: RxDma>(
    mmio: &mut M,
    descriptor_dma_base: u32,
    enable_rx: bool,
) -> Result<(), RxRingError> {
    let binding = RxDmaBinding::raw_validation(descriptor_dma_base);
    publish_cold_ring_inner(mmio, &binding, descriptor_dma_base, enable_rx)
}

/// Publish raw target addresses outside the owned storage path.
///
/// # Safety
///
/// `descriptor_dma_base` must identify a fully initialized static descriptor
/// chain whose buffers remain exclusively available to DMA until the walker
/// is confirmed stopped.
#[cfg(all(
    not(any(test, feature = "test-support")),
    feature = "validation-raw-dma"
))]
#[allow(
    unsafe_code,
    reason = "raw target publication makes its DMA lifetime contract explicit"
)]
pub unsafe fn publish_cold_ring<M: RxDma>(
    mmio: &mut M,
    descriptor_dma_base: u32,
    enable_rx: bool,
) -> Result<(), RxRingError> {
    let binding = RxDmaBinding::raw_validation(descriptor_dma_base);
    publish_cold_ring_inner(mmio, &binding, descriptor_dma_base, enable_rx)
}

fn publish_cold_ring_inner<M: RxDma>(
    mmio: &mut M,
    binding: &RxDmaBinding<'_>,
    descriptor_dma_base: u32,
    enable_rx: bool,
) -> Result<(), RxRingError> {
    if !descriptor_address_valid(descriptor_dma_base) {
        return Err(RxRingError::Address);
    }
    mmio.fence();
    mmio.configure_descriptor_window(binding);
    mmio.write_descriptor_base(binding, descriptor_dma_base);
    if enable_rx {
        mmio.publish_walker_enable(binding);
    }
    mmio.fence();
    Ok(())
}

/// Opens the RX walker in a native model after publishing a cold ring.
///
/// The vendor cold path keeps these operations separate:
/// `wDev_AppendRxBlocks` publishes the first descriptor base, while the later
/// `chip_enable` path calls `hal_mac_rx_enable`. Keeping this as a distinct
/// operation preserves that ordering and gives the base register time to
/// settle while the caller completes channel/MAC setup.
#[cfg(any(test, feature = "test-support"))]
pub fn enable_receive<M: RxDma>(mmio: &mut M) -> Result<(), RxRingError> {
    enable_receive_inner(mmio, &RxDmaBinding::raw_validation(0))
}

fn enable_receive_inner<M: RxDma>(
    mmio: &mut M,
    binding: &RxDmaBinding<'_>,
) -> Result<(), RxRingError> {
    mmio.try_with_walker_enabled(binding, |_enabled| ())
        .ok_or(RxRingError::Busy)
}

/// Stop the RX walker and confirm that the peripheral released its enable
/// edge before the owner rebuilds descriptor words or links.
///
/// The pinned `hal_mac_rx_disable` body is exactly this bit clear. The fence
/// and readback turn that raw leaf into an explicit Rust ownership boundary:
/// callers may mutate the ring only after this function returns `Ok(())`.
fn disable_receive_inner<M: RxDma>(mmio: &mut M) -> Result<(), RxRingError> {
    mmio.try_with_walker_stopped(|_stopped| ())
        .ok_or(RxRingError::Busy)
}

/// Stop a synthetic/native walker or an explicit raw-DMA validation epoch.
///
/// Production target lifecycle code must consume [`RxRingLive::try_stop`], so
/// the register edge and the Rust ownership state cannot diverge.
#[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
pub fn disable_receive<M: RxDma>(mmio: &mut M) -> Result<(), RxRingError> {
    disable_receive_inner(mmio)
}

/// Returns one CPU-owned completed descriptor to the cold/live ring.
#[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
pub fn rearm_descriptor(
    descriptor: &Descriptor,
    expected_buffer_address: u32,
    expected_next_address: u32,
) -> Result<(), RxRingError> {
    let word0 = descriptor.word0();
    let capacity = descriptor_size(word0);
    if descriptor.buffer_address() != expected_buffer_address
        || descriptor.next_address() != expected_next_address
        || !dma_range_valid(expected_buffer_address, capacity)
    {
        return Err(RxRingError::Corrupt);
    }
    if !rx_done(word0) {
        return Err(RxRingError::Busy);
    }
    descriptor.write_word0(rx_rearm_word(word0).ok_or(RxRingError::Size)?);
    Ok(())
}

#[cfg(test)]
mod observed_prefix_tests;
