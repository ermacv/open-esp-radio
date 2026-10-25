//! Permanently located storage for an ESP32-S31 RX DMA ring.
//!
//! This module owns only the chip DMA memory representation. RxDescriptor count,
//! payload capacity and placement policy are selected by the board or the
//! integrating firmware and remain const-generic here.

use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    ptr::NonNull,
    sync::atomic::{AtomicU8, Ordering},
};

use crate::dma_static::Placed;
use crate::layouts::{DmaWindowAddress, WifiRxDmaDescriptorControl};
use oer_memory::{ExternalRxBuffer, ExternalRxReturn, ExternalRxReturnState};

use crate::ieee80211::dma::{
    descriptor::RxDescriptor,
    rx_dma::{RxDescriptorAddresses, RxDma},
    rx_ring::{
        RX_BUFFER_SENTINEL, RxArenaWords, RxCompletedDescriptor, RxCompletedUnit,
        RxCompletedUnitFrontier, RxDescriptorTable, RxDmaArenaState, RxDmaBufferAddresses,
        RxFrozenCursor, RxLiveAppend, RxReservedBuffer, RxRingError, RxRingHalted, RxRingLive,
        RxRingPrepared, RxRingStopped, RxSegment, prepare_recycled_buffer,
    },
    space::{ChipDma, DmaSpace},
};

/// One aligned DMA-visible buffer with room for the hardware recycle guard.
#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
pub struct RxDmaBuffer<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>(
    UnsafeCell<[u8; STORAGE_SIZE]>,
    ExternalRxReturn,
);

impl<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE> {
    const fn new() -> Self {
        assert!(STORAGE_SIZE >= BUFFER_SIZE + 4);
        Self(UnsafeCell::new([0; STORAGE_SIZE]), ExternalRxReturn::new())
    }

    fn cpu_owned_mut(&mut self) -> &mut [u8; BUFFER_SIZE] {
        self.0
            .get_mut()
            .get_mut(..BUFFER_SIZE)
            .expect("RX DMA storage always contains its declared buffer prefix")
            .try_into()
            .expect("the selected RX DMA prefix has the exact declared size")
    }

    fn cpu_owned_storage_mut(&mut self) -> &mut [u8; STORAGE_SIZE] {
        self.0.get_mut()
    }

    /// # Safety
    ///
    /// The caller must own the matching completed descriptor. The returned
    /// view must not survive descriptor recycle.
    #[allow(unsafe_code, reason = "completed descriptor proves CPU ownership")]
    pub(crate) unsafe fn completed(&self) -> &[u8; BUFFER_SIZE] {
        // SAFETY: the type guarantees a prefix of exactly this size.
        unsafe { &*self.0.get().cast::<[u8; BUFFER_SIZE]>() }
    }

    /// # Safety
    ///
    /// The caller must own the matching completed descriptor and invoke this
    /// only from the ring's rearm closure.
    #[allow(unsafe_code, reason = "ring rearm owns the DMA buffer transition")]
    pub(crate) unsafe fn prepare_for_recycle(&self) -> Result<(), RxRingError> {
        match self.1.state() {
            ExternalRxReturnState::Attached => {}
            ExternalRxReturnState::Detached | ExternalRxReturnState::Returned => {
                return Err(RxRingError::BufferOwnershipConflict);
            }
        }
        // SAFETY: ring ownership makes this the only CPU or DMA writer.
        unsafe { prepare_recycled_buffer(&mut *self.0.get(), BUFFER_SIZE) }
    }

    /// Normalize an observed buffer before a role-neutral frontier republishes
    /// its descriptor.
    ///
    /// A staged role may hand a live ring to a finite scan/join frontier after
    /// every upper lease has returned but before the next staged service pass
    /// recycled the released prefix. The descriptor remains observed and is
    /// therefore safe to republish, but its allocation still carries the
    /// durable `Returned` state. Normalize that state in the same transaction;
    /// otherwise the descriptor becomes DMA-visible while a later detach sees
    /// the stale release and rejects a real completion as `BufferOwnershipConflict`.
    #[allow(
        unsafe_code,
        reason = "the observed live-ring prefix excludes DMA and detached upper ownership"
    )]
    unsafe fn prepare_observed_for_recycle(&self) -> Result<(), RxRingError> {
        self.1
            .reattach()
            .map_err(|_| RxRingError::BufferOwnershipConflict)?;
        // SAFETY: the state check/transition excludes every external owner.
        unsafe { self.prepare_for_recycle() }
    }

    fn detach_with(&'static self, length: usize) -> Result<ExternalRxBuffer, RxRingError> {
        if length == 0 || length > BUFFER_SIZE {
            return Err(RxRingError::Size);
        }
        let pointer = NonNull::new(self.0.get().cast::<u8>()).expect("RX DMA buffer is non-null");
        #[allow(
            unsafe_code,
            reason = "completed descriptor detaches stable DMA buffer ownership"
        )]
        // SAFETY: the completed-unit owner proved that DMA released this
        // buffer. The buffer is part of stable arena storage and the state
        // transition prevents recycle until the affine buffer is returned.
        unsafe { self.1.detach(pointer, length, BUFFER_SIZE) }
            .map_err(|_| RxRingError::BufferOwnershipConflict)
    }

    fn is_released(&self) -> bool {
        self.1.state() == ExternalRxReturnState::Returned
    }

    fn state(&self) -> ExternalRxReturnState {
        self.1.state()
    }

    #[allow(
        unsafe_code,
        reason = "stopped DMA owner may normalize a returned detached buffer"
    )]
    unsafe fn prepare_for_stopped_ring(&self) -> Result<(), RxRingError> {
        self.1
            .reattach()
            .map_err(|_| RxRingError::BufferOwnershipConflict)?;
        // SAFETY: stopped-ring ownership excludes DMA, and DETACHED was
        // rejected before touching the allocation.
        unsafe { self.prepare_for_recycle() }
    }

    /// Whether DMA has overwritten the leading recycle guard.
    ///
    /// This is observation only: it never transfers buffer ownership. It is
    /// used together with a later terminal descriptor to distinguish a full
    /// non-terminal segment from an untouched armed descriptor.
    #[allow(
        unsafe_code,
        reason = "volatile read observes the asynchronous DMA writer"
    )]
    pub(crate) fn leading_guard_overwritten(&self) -> bool {
        // SAFETY: volatile access models the asynchronous DMA writer. The
        // result is only evidence for a subsequent descriptor observation.
        unsafe { self.0.get().cast::<u32>().read_volatile() != RX_BUFFER_SENTINEL }
    }
}

#[allow(
    unsafe_code,
    reason = "detached-buffer state machine serializes cross-core access"
)]
// SAFETY: the atomic detached-buffer state serializes every cross-core
// access to the byte cell; only the current state owner reads or writes it.
unsafe impl<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> Sync
    for RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>
{
}

/// The reserved descriptor's buffer lent to its live ring.
struct ReservedBufferPort<'a, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>(
    &'a RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>,
);

impl<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> RxReservedBuffer
    for ReservedBufferPort<'_, BUFFER_SIZE, STORAGE_SIZE>
{
    fn guard_overwritten(&self) -> bool {
        self.0.leading_guard_overwritten()
    }

    #[allow(
        unsafe_code,
        reason = "the live ring rearms the reserved buffer only after the walker finished it"
    )]
    fn restore_guards(&self) -> Result<(), RxRingError> {
        // SAFETY: the reserved buffer is never detached or leased, so the
        // ring is its only CPU owner, and the ring calls this only after the
        // walker finished writing it (`RX_DONE` on R, or a later ring
        // descriptor completed).
        unsafe { self.0.prepare_for_recycle() }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxDmaStorageError {
    AddressWidth,
    Count,
}

/// Read authority for one completed descriptor and its matching DMA buffer.
///
/// The token retains the unique mutable borrow of the live ring. Consequently
/// no safe caller can rearm the descriptor while a segment view is alive.
pub struct RxDmaCompletedDescriptor<
    'owner,
    'ring,
    const COUNT: usize,
    const BUFFER_SIZE: usize,
    const STORAGE_SIZE: usize,
> {
    descriptor: RxCompletedDescriptor,
    buffer: &'owner RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>,
    _ring: &'owner mut RxRingLive<'ring, COUNT>,
    // A hardware completion transaction belongs to the core-local DMA owner.
    // Only the detached packet buffer produced by that transaction may cross
    // an executor/core boundary.
    _not_send: PhantomData<*mut ()>,
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>
    RxDmaCompletedDescriptor<'_, '_, COUNT, BUFFER_SIZE, STORAGE_SIZE>
{
    pub const fn index(&self) -> usize {
        self.descriptor.index()
    }

    #[allow(
        unsafe_code,
        reason = "completed descriptor token retains the live ring"
    )]
    pub fn segment(&self) -> RxSegment<'_> {
        // SAFETY: the unforgeable descriptor token retains the live ring's
        // mutable borrow, and `validate_live_ring` matched this buffer arena
        // to that ring before transferring completion ownership.
        let buffer = unsafe { self.buffer.completed() };
        RxSegment {
            descriptor_address: self.descriptor.descriptor_address(),
            descriptor_word0: self.descriptor.word0(),
            buffer,
            next_descriptor_address: self.descriptor.next_descriptor_address(),
        }
    }
}

/// Read and recycle authority for one complete, possibly chained RX unit.
///
/// All segment views are bounded by lengths captured before this token was
/// created. The token retains the unique live-ring borrow until the complete
/// copy is finished and [`recycle`](Self::recycle) consumes it.
pub struct RxDmaCompletedUnit<
    'storage,
    'owner,
    'ring,
    const COUNT: usize,
    const BUFFER_SIZE: usize,
    const STORAGE_SIZE: usize,
    S: DmaSpace = ChipDma,
> {
    unit: RxCompletedUnit,
    storage: &'storage RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE, S>,
    ring: Option<&'owner mut RxRingLive<'ring, COUNT>>,
    requires_recycle: bool,
    // Keep physical descriptor credit on the core-local DMA owner. The
    // consuming detach transition deliberately returns a separate Send-able
    // packet-buffer type.
    _not_send: PhantomData<*mut ()>,
}

/// One complete single-buffer unit detached from its descriptor.
pub struct RxDmaDetachedUnit {
    buffer: ExternalRxBuffer,
    descriptor_address: u32,
    descriptor_word0: WifiRxDmaDescriptorControl,
    length: usize,
}

/// Affine permission to append one observed unit at the recycle frontier.
///
/// Its exclusive ring loan keeps the frozen LAST generation current. Neither
/// a descriptor count nor a copied cursor can publish descriptors on its own.
///
/// ```compile_fail,E0382
/// use oer_esp32s31_hal::ieee80211::dma::{rx_dma::RxDma, rx_storage::RxDmaRecycleUnit};
/// fn duplicate(credit: RxDmaRecycleUnit<'_, '_, '_, 4, 16, 20>, mmio: &mut impl RxDma) {
///     let _ = credit.append(mmio);
///     let _ = credit.append(mmio);
/// }
/// ```
///
/// ```compile_fail,E0499
/// use oer_esp32s31_hal::ieee80211::dma::{
///     rx_dma::RxDma, rx_ring::{RxRingLive, RxFrozenCursor}, rx_storage::RxDmaStorage,
/// };
/// fn change_generation(storage: &RxDmaStorage<4, 16, 20>, ring: &mut RxRingLive<'_, 4>,
///                      cursor: RxFrozenCursor, mmio: &mut impl RxDma) {
///     let credit = storage.claim_deferred_recycle(ring, cursor).unwrap().unwrap();
///     let _ = ring.freeze_cursor(mmio);
///     let _ = credit.append(mmio);
/// }
/// ```
#[must_use = "append the unit or retain it for a later recycle pass"]
pub struct RxDmaRecycleUnit<
    'storage,
    'owner,
    'ring,
    const COUNT: usize,
    const BUFFER_SIZE: usize,
    const STORAGE_SIZE: usize,
    S: DmaSpace = ChipDma,
> {
    storage: &'storage RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE, S>,
    ring: &'owner mut RxRingLive<'ring, COUNT>,
    descriptor_count: usize,
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize, S: DmaSpace>
    RxDmaRecycleUnit<'_, '_, '_, COUNT, BUFFER_SIZE, STORAGE_SIZE, S>
{
    pub const fn descriptor_count(&self) -> usize {
        self.descriptor_count
    }

    #[allow(
        unsafe_code,
        reason = "the affine unit retains exclusive buffer rearm authority"
    )]
    pub fn append<M: RxDma>(self, mmio: &mut M) -> Result<RxLiveAppend<COUNT>, RxRingError> {
        self.ring
            .recycle_proven_unit_owned(mmio, self.descriptor_count, |index| {
                // SAFETY: the token's ring loan retains the observed complete
                // unit and its frozen link-release proof until this publication.
                unsafe {
                    self.storage
                        .buffer_for_descriptor(index)
                        .ok_or(RxRingError::Count)?
                        .prepare_for_recycle()
                }
            })
    }
}

impl RxDmaDetachedUnit {
    pub const fn descriptor_address(&self) -> u32 {
        self.descriptor_address
    }

    pub const fn descriptor_word0(&self) -> WifiRxDmaDescriptorControl {
        self.descriptor_word0
    }

    pub const fn length(&self) -> usize {
        self.length
    }

    pub fn into_buffer(self) -> ExternalRxBuffer {
        self.buffer
    }
}

impl<
    'storage,
    'owner,
    'ring,
    const COUNT: usize,
    const BUFFER_SIZE: usize,
    const STORAGE_SIZE: usize,
    S: DmaSpace,
> RxDmaCompletedUnit<'storage, 'owner, 'ring, COUNT, BUFFER_SIZE, STORAGE_SIZE, S>
{
    pub const fn head_index(&self) -> usize {
        self.unit.head_index()
    }

    pub const fn descriptor_count(&self) -> usize {
        self.unit.descriptor_count()
    }

    pub const fn total_length(&self) -> usize {
        self.unit.total_length()
    }

    pub const fn metadata(&self) -> &RxCompletedUnit {
        &self.unit
    }

    #[allow(unsafe_code, reason = "completed unit token retains every segment")]
    pub fn segment(&self, step: usize) -> Option<&[u8]> {
        let length = self.unit.segment_length(step)?;
        let index = self
            .unit
            .head_index()
            .checked_add(step)?
            .checked_rem(COUNT)?;
        let buffer = self.storage.buffer_for_descriptor(index)?;
        // SAFETY: `RxCompletedUnit` proves that the terminal descriptor made
        // every preceding segment CPU-owned. This token retains the mutable
        // ring borrow, preventing recycle while the returned slice lives.
        unsafe { buffer.completed().get(..length) }
    }

    #[allow(
        unsafe_code,
        reason = "consuming completed unit owns the rearm transition"
    )]
    pub fn recycle<M: RxDma>(
        mut self,
        mmio: &mut M,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError> {
        let descriptor_count = self.unit.descriptor_count();
        let result = self
            .ring
            .as_deref_mut()
            .expect("completed unit retains ring loan")
            .recycle_completed_unit_owned(mmio, descriptor_count, |index| {
                // SAFETY: this consuming token owns every segment until this
                // rearm closure returns it to the live walker.
                unsafe {
                    self.storage
                        .buffer_for_descriptor(index)
                        .ok_or(RxRingError::Count)?
                        .prepare_for_recycle()
                }
            });
        if matches!(result, Ok(Some(_))) {
            self.requires_recycle = false;
        }
        self.storage.observe_append(result)
    }

    /// Finish the staging handoff without returning this descriptor unit to
    /// the active walker yet.
    ///
    /// The unit remains represented by the ring's observed mask. Its producer
    /// must reclaim it against the same frozen LAST before allowing an append
    /// to create a new descriptor-address generation.
    pub fn retain_for_deferred_recycle(mut self) {
        self.requires_recycle = false;
    }

    /// Retain a discard, returning append authority only when this unit is
    /// already at the recycle frontier. A preceding detached upper owner
    /// leaves it deferred; that owner's release must advance the frontier.
    pub fn retain_discard(
        mut self,
    ) -> Option<RxDmaRecycleUnit<'storage, 'owner, 'ring, COUNT, BUFFER_SIZE, STORAGE_SIZE, S>>
    {
        self.requires_recycle = false;
        if self.unit.head_index()
            != self
                .ring
                .as_deref()
                .expect("completed unit retains ring loan")
                .recycle_start()
                .get()
        {
            return None;
        }
        let descriptor_count = self.unit.descriptor_count();
        let ring = self.ring.take().expect("completed unit retains ring loan");
        Some(RxDmaRecycleUnit {
            storage: self.storage,
            ring,
            descriptor_count,
        })
    }

    /// Detach one complete hardware buffer for zero-copy upper ownership.
    ///
    /// The descriptor remains observed and CPU-owned in the ring. Before the
    /// buffer is returned, the descriptor's owner bit is cleared and `mmio`
    /// fences, so a walker that ever reached it would stop on it instead of
    /// writing the leased buffer (the second layer next to the reserved
    /// descriptor, [`RxRingLive`]). Dropping the returned buffer marks it
    /// released; only a later ring-owner service may rearm the descriptor,
    /// which sets the owner bit again.
    pub fn detach_single<M: RxDma>(mut self, mmio: &mut M) -> Result<RxDmaDetachedUnit, RxRingError>
    where
        'storage: 'static,
    {
        if self.unit.descriptor_count() != 1 {
            return Err(RxRingError::Count);
        }
        // A single descriptor's length always fits its own length field.
        let descriptor_word0 = self.unit.staged_word0().ok_or(RxRingError::Size)?;
        let index = self.unit.head_index();
        let buffer_id = self
            .storage
            .descriptor_buffer_id(index)
            .ok_or(RxRingError::Count)?;
        let detached_word = self
            .ring
            .as_deref_mut()
            .expect("completed unit retains ring loan")
            .detached_word(index)?;
        let buffer = self
            .storage
            .detach_buffer(self.unit.total_length(), buffer_id)?;
        self.ring
            .as_deref_mut()
            .expect("completed unit retains ring loan")
            .publish_detached(mmio, index, detached_word);
        self.requires_recycle = false;
        Ok(RxDmaDetachedUnit {
            buffer,
            descriptor_address: self.unit.descriptor_address(),
            descriptor_word0,
            length: self.unit.total_length(),
        })
    }
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize, S: DmaSpace> Drop
    for RxDmaCompletedUnit<'_, '_, '_, COUNT, BUFFER_SIZE, STORAGE_SIZE, S>
{
    fn drop(&mut self) {
        if self.requires_recycle {
            // `take_completed_unit` marks the complete descriptor group as
            // observed exactly once. Losing that linear CPU owner before a
            // successful rearm would otherwise leave an arena which looks
            // live but can never transfer the same group again.
            self.ring
                .as_deref_mut()
                .expect("completed unit retains ring loan")
                .require_reset();
        }
    }
}

/// RxDescriptor and buffer arena whose address remains stable for a DMA epoch.
///
/// The buffer address table remains caller-owned because the live ring borrows
/// it for its entire epoch. Keeping that table separate avoids a
/// self-referential owner and lets a platform place only DMA-visible storage
/// in its dedicated linker section.
///
/// Its zero bytes are the reusable arena `new` builds: empty descriptors,
/// buffers in the ring, the identity descriptor binding and no observer.
#[repr(C)]
#[derive(bytemuck::Zeroable)]
#[zeroable(bound = "")]
pub struct RxDmaStorage<
    const COUNT: usize,
    const BUFFER_SIZE: usize,
    const STORAGE_SIZE: usize,
    S: DmaSpace = ChipDma,
> {
    /// The ring and, right after it, the reserved descriptor R.
    descriptors: RxDescriptorTable<COUNT>,
    buffers: [RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>; COUNT],
    /// R's buffer: never one of `buffers`, never detached or leased.
    reserved_buffer: RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>,
    /// Per descriptor, the distance to its bound buffer: descriptor `i` uses
    /// buffer `(i + offset) % COUNT`. Zero is the identity binding, so the
    /// arena's zero bytes bind every descriptor to its own buffer.
    descriptor_buffer_offsets: [AtomicU8; COUNT],
    /// Per descriptor, the DMA address of its bound buffer, written once
    /// when the storage is placed ([`RxDmaStorage::place`]).
    buffer_addresses: RxDmaBufferAddresses<COUNT>,
    arena: RxArenaWords,
    space: PhantomData<S>,
}

#[allow(unsafe_code, reason = "atomic bindings serialize cross-core ownership")]
// SAFETY: descriptors, buffers and bindings change only through their
// atomic ownership bindings and lifecycle state, which serialize every
// cross-core access.
unsafe impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize, S: DmaSpace>
    Sync for RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE, S>
{
}

/// An RX DMA storage placed in the DMA window ([`RxDmaStorage::place`]).
///
/// It is the only way to a ring: the descriptor base and the buffer
/// addresses it publishes are the placed storage's own, so the link's
/// assertions over the DMA range (`dma-window.x`) prove them, and the ring
/// checks no address against the window at run time. A [`Placed`] value is
/// a claimed `'static`, so temporary storage cannot authorize a DMA
/// lifecycle.
pub struct PlacedRxDma<
    const COUNT: usize,
    const BUFFER_SIZE: usize,
    const STORAGE_SIZE: usize,
    S: DmaSpace = ChipDma,
> {
    storage: &'static RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE, S>,
    /// The typed DMA addresses of the storage's ring and R.
    descriptors: RxDescriptorAddresses,
    /// The typed DMA address of R's buffer.
    reserved_buffer: DmaWindowAddress,
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize, S: DmaSpace>
    PlacedRxDma<COUNT, BUFFER_SIZE, STORAGE_SIZE, S>
{
    /// The placed storage.
    pub const fn storage(&self) -> &'static RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE, S> {
        self.storage
    }

    /// The DMA address of descriptor 0.
    pub const fn descriptor_base(&self) -> u32 {
        self.descriptors.base()
    }

    /// Prepare the first ring in memory without publishing BASE.
    /// The returned proof must pass through MAC core preparation and BASE
    /// publication before a walker can be started.
    ///
    /// The storage is `'static`: safe code may forget a live ring, so the
    /// DMA-visible descriptors and buffers stay allocated even when the
    /// typestate owner is lost.
    ///
    /// # Errors
    ///
    /// The arena must be reset, or an unexpectedly live walker did not stop.
    /// The unique placement owner is consumed even when preparation fails.
    ///
    /// ```compile_fail,E0382
    /// use oer_esp32s31_hal::ieee80211::dma::{rx_storage::PlacedRxDma, rx_dma::RxDma};
    /// fn twice<M: RxDma>(placed: PlacedRxDma<2, 16, 20>, mmio: &mut M) {
    ///     let _ = placed.prepare_ring(mmio);
    ///     let _ = placed.prepare_ring(mmio);
    /// }
    /// ```
    pub fn prepare_ring<M: RxDma>(
        self,
        mmio: &mut M,
    ) -> Result<RxRingPrepared<'static, COUNT>, RxRingError> {
        self.storage
            .prepare_ring_bound(mmio, self.descriptors, self.reserved_buffer)
    }
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize, S: DmaSpace>
    RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE, S>
{
    pub const fn new() -> Self {
        Self {
            descriptors: RxDescriptorTable::new(),
            buffers: [const { RxDmaBuffer::new() }; COUNT],
            reserved_buffer: RxDmaBuffer::new(),
            descriptor_buffer_offsets: [const { AtomicU8::new(0) }; COUNT],
            buffer_addresses: RxDmaBufferAddresses::new(),
            arena: RxArenaWords::new(),
            space: PhantomData,
        }
    }

    fn detach_buffer(
        &'static self,
        length: usize,
        index: usize,
    ) -> Result<ExternalRxBuffer, RxRingError> {
        let buffer = self.buffer_by_id(index).ok_or(RxRingError::Count)?;
        buffer.detach_with(length)
    }

    fn observe_append(
        &self,
        result: Result<Option<RxLiveAppend<COUNT>>, RxRingError>,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError> {
        result
    }

    /// Per descriptor, the DMA address of its bound buffer, as placing the
    /// storage recorded it (zero before).
    pub const fn buffer_addresses(&self) -> &RxDmaBufferAddresses<COUNT> {
        &self.buffer_addresses
    }

    /// Sticky lifecycle state stored with the static arena rather than in its
    /// movable ring token.
    pub fn lifecycle_state(&self) -> RxDmaArenaState {
        RxDmaArenaState::load(self.arena.lifecycle())
    }

    /// The ring's descriptors; the reserved descriptor is
    /// [`Self::reserved_descriptor`].
    pub const fn descriptors(&self) -> &[RxDescriptor; COUNT] {
        self.descriptors.ring()
    }

    /// The reserved descriptor R, the one `RX_DESCRIPTOR_BASE` names.
    pub const fn reserved_descriptor(&self) -> &RxDescriptor {
        self.descriptors.reserved()
    }

    fn reserved_port(&self) -> ReservedBufferPort<'_, BUFFER_SIZE, STORAGE_SIZE> {
        ReservedBufferPort(&self.reserved_buffer)
    }

    /// The descriptor table a model walker lands frames in and resolves its
    /// cursors against (test support only); `None` when the arena has no
    /// DMA address in its space.
    #[cfg(any(test, feature = "test-support"))]
    pub fn walker_table(&self) -> Option<crate::ieee80211::dma::rx_dma::RxWalkerTable<'_>> {
        let base = self.descriptor_base_address().ok()?;
        Some(crate::ieee80211::dma::rx_dma::RxWalkerTable::new(
            base,
            self.descriptors.ring(),
            self.descriptors.reserved(),
        ))
    }

    pub const fn buffers(&self) -> &[RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>; COUNT] {
        &self.buffers
    }

    /// Mutable CPU view available only before an RX ring borrows this arena.
    ///
    /// The exclusive storage borrow prevents a stopped or live descriptor
    /// owner from existing at the same time. This is primarily useful for
    /// deterministic host fixtures and pre-publication initialization.
    pub fn buffer_mut(&mut self, index: usize) -> Option<&mut [u8; BUFFER_SIZE]> {
        self.buffers.get_mut(index).map(RxDmaBuffer::cpu_owned_mut)
    }

    /// Restore one buffer's recycle guards before any ring borrows this arena.
    ///
    /// Requiring `&mut self` proves that no stopped/live ring token can exist
    /// at the same time, so pre-publication setup does not need privileged
    /// access to the buffer's `UnsafeCell`.
    pub fn prepare_unpublished_buffer(&mut self, index: usize) -> Result<(), RxRingError> {
        let buffer = self.buffers.get_mut(index).ok_or(RxRingError::Count)?;
        prepare_recycled_buffer(buffer.cpu_owned_storage_mut(), BUFFER_SIZE)
    }

    /// Place the storage: bind each descriptor to its buffer and record the
    /// buffers' DMA addresses. Only a placed storage prepares a ring, so
    /// every address the ring publishes lies in the DMA window by the link's
    /// proof (`dma-window.x`).
    ///
    /// # Errors
    ///
    /// An address does not fit the DMA address width of the space.
    pub fn place(
        placed: Placed<Self>,
    ) -> Result<PlacedRxDma<COUNT, BUFFER_SIZE, STORAGE_SIZE, S>, RxDmaStorageError> {
        Self::place_rotated(placed, 0)
    }

    /// [`Self::place`] with each physical descriptor bound to a cyclically
    /// shifted buffer.
    ///
    /// Rotation zero is the production identity layout. A non-zero rotation
    /// is a target proof that every CPU ownership path follows the explicit
    /// descriptor-to-buffer binding instead of indexing the buffer arena by
    /// descriptor number. The binding is frozen before the first ring owner
    /// exists and remains immutable for that complete DMA lifecycle.
    ///
    /// # Errors
    ///
    /// The rotation is not below `COUNT`, or an address does not fit the
    /// DMA address width of the space.
    pub fn place_rotated(
        placed: Placed<Self>,
        rotation: usize,
    ) -> Result<PlacedRxDma<COUNT, BUFFER_SIZE, STORAGE_SIZE, S>, RxDmaStorageError> {
        let storage = placed.into_inner();
        storage.validate_descriptor_rotation(rotation)?;
        for descriptor_index in 0..COUNT {
            let buffer_id = (descriptor_index + rotation) % COUNT;
            let address = storage.placed_buffer_address(&storage.buffers[buffer_id])?;
            storage.buffer_addresses.record(descriptor_index, address);
        }
        storage.bind_descriptor_rotation(rotation)?;
        let base = storage.descriptor_base_address()?;
        // SAFETY: `base` is the DMA address of this placed storage's
        // descriptor table: COUNT ring descriptors and R behind them,
        // 12-byte and aligned (`RxDescriptorTable` is `repr(C)`). The
        // consumed `Placed` proves the static lies in the DMA window, and
        // the `'static` storage stays allocated for every DMA epoch.
        #[allow(unsafe_code, reason = "the placed RX arena's descriptor table")]
        let descriptors = unsafe { RxDescriptorAddresses::new(base, COUNT + 1) }
            .ok_or(RxDmaStorageError::AddressWidth)?;
        let reserved_buffer = storage.placed_buffer_address(&storage.reserved_buffer)?;
        Ok(PlacedRxDma {
            storage,
            descriptors,
            reserved_buffer,
        })
    }

    fn validate_descriptor_rotation(&self, rotation: usize) -> Result<(), RxDmaStorageError> {
        if COUNT == 0 || COUNT > usize::from(u8::MAX) + 1 || rotation >= COUNT {
            return Err(RxDmaStorageError::Count);
        }
        if self.lifecycle_state() != RxDmaArenaState::Reusable {
            return Err(RxDmaStorageError::Count);
        }
        Ok(())
    }

    #[allow(
        unsafe_code,
        reason = "Reusable lifecycle is the exclusive pre-ring binding boundary"
    )]
    fn bind_descriptor_rotation(&self, rotation: usize) -> Result<(), RxDmaStorageError> {
        self.validate_descriptor_rotation(rotation)?;
        for offset in &self.descriptor_buffer_offsets {
            offset.store(rotation as u8, Ordering::Release);
        }
        Ok(())
    }

    /// Current physical buffer identity bound to one descriptor.
    pub fn descriptor_buffer_id(&self, descriptor_index: usize) -> Option<usize> {
        self.descriptor_buffer_offsets
            .get(descriptor_index)
            .map(|offset| (descriptor_index + usize::from(offset.load(Ordering::Acquire))) % COUNT)
    }

    fn buffer_for_descriptor(
        &self,
        descriptor_index: usize,
    ) -> Option<&RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>> {
        self.buffer_by_id(self.descriptor_buffer_id(descriptor_index)?)
    }

    fn any_buffer_in_state(&self, state: ExternalRxReturnState) -> bool {
        self.buffers.iter().any(|buffer| buffer.state() == state)
    }

    pub fn detached_buffer_count(&self) -> usize {
        self.buffers
            .iter()
            .filter(|buffer| buffer.state() == ExternalRxReturnState::Detached)
            .count()
    }

    fn buffer_by_id(&self, buffer_id: usize) -> Option<&RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>> {
        self.buffers.get(buffer_id)
    }

    #[allow(
        unsafe_code,
        reason = "stopped ring exclusively owns initial buffer rearm"
    )]
    fn prepare_ring_bound<'storage, M: RxDma>(
        &'storage self,
        mmio: &mut M,
        descriptors: RxDescriptorAddresses,
        reserved_buffer: DmaWindowAddress,
    ) -> Result<RxRingPrepared<'storage, COUNT>, RxRingError> {
        let buffer_addresses = &self.buffer_addresses;
        match self.lifecycle_state() {
            RxDmaArenaState::Reusable => {}
            RxDmaArenaState::Prepared | RxDmaArenaState::Live => {
                panic!("RX initial owner invariant");
            }
            RxDmaArenaState::ResetRequired => return Err(RxRingError::ResetRequired),
        }
        let buffer_size = u32::try_from(BUFFER_SIZE).map_err(|_| RxRingError::Size)?;
        // The consumed, non-Copy placement owner is the sole constructor.
        // Keep the durable claim before publishing descriptors, so a lost
        // owner or failed preparation still leaves the arena fail-closed.
        assert!(
            RxDmaArenaState::claim_prepared(self.arena.lifecycle()),
            "RX initial owner invariant"
        );
        let prepared = RxRingStopped::prepare_inner(
            mmio,
            &self.descriptors,
            descriptors,
            buffer_addresses,
            reserved_buffer,
            buffer_size,
            |index| {
                // SAFETY: stopped-ring preparation owns every descriptor and
                // the storage's own address table binds `index` to this
                // arena.
                unsafe { self.prepare_stopped_buffer(index) }
            },
            || {
                // SAFETY: stopped-ring preparation owns R and its buffer.
                unsafe { self.reserved_buffer.prepare_for_stopped_ring() }
            },
            &self.arena,
        );
        if prepared.is_err() {
            // Preparation may have failed while disabling an unexpectedly
            // live walker. Without the returned ring capability there is no
            // proof that DMA stopped referencing this arena, so never reopen
            // the root constructor until radio reset.
            RxDmaArenaState::ResetRequired.store(self.arena.lifecycle());
        }
        prepared
    }

    /// Rebuild a halted ring only when it belongs to this storage arena.
    #[allow(unsafe_code, reason = "halted ring exclusively owns buffer rearm")]
    pub fn prepare_halted<'storage, M: RxDma>(
        &'storage self,
        ring: RxRingHalted<'storage, COUNT>,
        mmio: &mut M,
    ) -> Result<RxRingStopped<'storage, COUNT>, (RxRingHalted<'storage, COUNT>, RxRingError)> {
        if !ring.belongs_to_arena(&self.arena) {
            return Err((ring, RxRingError::DescriptorOwnerAddress));
        }
        if self.any_buffer_in_state(ExternalRxReturnState::Detached) {
            return Err((ring, RxRingError::OutstandingLeases));
        }
        match self.lifecycle_state() {
            RxDmaArenaState::Prepared => {}
            RxDmaArenaState::ResetRequired => {
                return Err((ring, RxRingError::ResetRequired));
            }
            RxDmaArenaState::Reusable | RxDmaArenaState::Live => {
                panic!("RX halted owner invariant");
            }
        }
        let buffer_size = match u32::try_from(BUFFER_SIZE) {
            Ok(buffer_size) => buffer_size,
            Err(_) => return Err((ring, RxRingError::Size)),
        };
        ring.prepare_owned(
            mmio,
            buffer_size,
            |index| {
                // SAFETY: the halted owner proves that DMA is stopped, and it
                // belongs to this arena, whose layout binds the descriptor to
                // this exact buffer.
                unsafe { self.prepare_stopped_buffer(index) }
            },
            || {
                // SAFETY: the halted owner proves that DMA is stopped; R's
                // buffer belongs to this arena.
                unsafe { self.reserved_buffer.prepare_for_stopped_ring() }
            },
        )
    }

    /// # Safety
    /// Called only by a stopped/halted ring's exclusive preparation closure.
    #[allow(
        unsafe_code,
        reason = "stopped ring preparation owns this physical buffer"
    )]
    unsafe fn prepare_stopped_buffer(&self, index: usize) -> Result<(), RxRingError> {
        let buffer = self
            .buffer_for_descriptor(index)
            .ok_or(RxRingError::Count)?;
        // SAFETY: the enclosing ring preparation closure proves DMA stopped.
        unsafe { buffer.prepare_for_stopped_ring()? };
        Ok(())
    }

    /// Transfer one completed descriptor together with its buffer read
    /// authority. The returned token prevents a concurrent recycle borrow.
    ///
    /// Only a descriptor from the recycle frontier through the LAST of
    /// `cursor` inclusive is taken: the walker no longer writes it. A
    /// descriptor past that LAST, or a cursor of another ring, live epoch or
    /// descriptor generation, takes nothing, before any descriptor image is
    /// read.
    pub fn take_completed<'owner, 'ring>(
        &'owner self,
        ring: &'owner mut RxRingLive<'ring, COUNT>,
        cursor: RxFrozenCursor,
        index: usize,
    ) -> Result<
        Option<RxDmaCompletedDescriptor<'owner, 'ring, COUNT, BUFFER_SIZE, STORAGE_SIZE>>,
        RxRingError,
    > {
        self.validate_live_ring(ring)?;
        if index >= COUNT {
            return Err(RxRingError::Count);
        }
        if !ring.frozen_cursor_releases_index(cursor, index) {
            return Ok(None);
        }
        match ring.validate_completed_descriptor(index) {
            Ok(true) => {}
            Ok(false) => return Ok(None),
            Err(error) => {
                if error != RxRingError::Count {
                    ring.require_reset();
                }
                return Err(error);
            }
        }
        let Some(descriptor) = ring.take_completed_owned(index, &self.reserved_port()) else {
            return Ok(None);
        };
        let buffer = self
            .buffer_for_descriptor(index)
            .ok_or(RxRingError::Count)?;
        Ok(Some(RxDmaCompletedDescriptor {
            descriptor,
            buffer,
            _ring: ring,
            _not_send: PhantomData,
        }))
    }

    #[allow(
        unsafe_code,
        reason = "live ring proves its completed half is CPU-owned"
    )]
    pub fn recycle_completed_half<M: RxDma>(
        &self,
        ring: &mut RxRingLive<'_, COUNT>,
        mmio: &mut M,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError> {
        self.validate_live_ring(ring)?;
        let result = ring.recycle_completed_half_owned(mmio, |index| {
            // SAFETY: the ring invokes the closure only after observing the
            // complete half and immediately before descriptor publication.
            unsafe {
                self.buffer_for_descriptor(index)
                    .ok_or(RxRingError::Count)?
                    .prepare_observed_for_recycle()
            }
        });
        self.observe_append(result)
    }

    #[allow(
        unsafe_code,
        reason = "live ring proves its completed prefix is CPU-owned"
    )]
    pub fn recycle_completed_prefix<const MAX_BATCH: usize, M: RxDma>(
        &self,
        ring: &mut RxRingLive<'_, COUNT>,
        mmio: &mut M,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError> {
        self.validate_live_ring(ring)?;
        let result = ring.recycle_completed_prefix_owned::<MAX_BATCH, _, _>(mmio, |index| {
            // SAFETY: the ring invokes the closure only for its observed
            // prefix immediately before returning those buffers to DMA.
            unsafe {
                self.buffer_for_descriptor(index)
                    .ok_or(RxRingError::Count)?
                    .prepare_observed_for_recycle()
            }
        });
        self.observe_append(result)
    }

    /// Return the longest contiguous prefix whose detached upper owners have
    /// all released their original DMA buffers.
    ///
    /// A still-retained descriptor terminates the prefix even if later frames
    /// have already returned. Ring-owned staged discards are deliberately not
    /// included: after the released prefix advances, the caller must bind each
    /// such unit to a fresh frozen-LAST proof before republishing it.
    #[allow(
        unsafe_code,
        reason = "released buffer state authorizes the ring rearm transition"
    )]
    pub fn recycle_released_prefix<const MAX_BATCH: usize, M: RxDma>(
        &self,
        ring: &mut RxRingLive<'_, COUNT>,
        mmio: &mut M,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError> {
        self.validate_live_ring(ring)?;
        if MAX_BATCH == 0 {
            return Err(RxRingError::Count);
        }
        let start = ring.recycle_start();
        let observed = ring.observed_mask();
        let mut released = 0_usize;
        while released < MAX_BATCH.min(COUNT) {
            let index = start.advance(released).get();
            if !observed.contains(index)
                || !self.descriptors()[index].word0().rx_done()
                || !self
                    .buffer_for_descriptor(index)
                    .ok_or(RxRingError::Count)?
                    .is_released()
            {
                break;
            }
            released += 1;
        }
        if released == 0 {
            return Ok(None);
        }
        let result = ring.recycle_released_terminal_prefix_owned(mmio, released, |index| {
            // SAFETY: the prefix scan proved RELEASED and the ring revalidates
            // the complete descriptor image before invoking this callback.
            unsafe {
                self.buffer_for_descriptor(index)
                    .ok_or(RxRingError::Count)?
                    .prepare_observed_for_recycle()
            }
        });
        self.observe_append(result)
    }

    /// Number of detached buffers already returned by upper owners but not
    /// yet appended back to the hardware list.
    pub fn released_buffer_count(&self) -> usize {
        self.buffers
            .iter()
            .filter(|buffer| buffer.is_released())
            .count()
    }

    /// Return the descriptor count of the first already-observed unit when it
    /// never detached from the ring.
    ///
    /// This is the deferred-discard counterpart to released-prefix reclaim. A
    /// returned network buffer (`Returned`) and a live upper lease
    /// (`Detached`) are excluded; the caller must first advance those owners
    /// in ring order. A terminal is required so a partial chain can never be
    /// republished.
    pub fn first_observed_ring_unit_descriptor_count(
        &self,
        ring: &RxRingLive<'_, COUNT>,
    ) -> Result<Option<usize>, RxRingError> {
        self.validate_live_ring(ring)?;
        let start = ring.recycle_start();
        let observed = ring.observed_mask();
        for step in 0..COUNT.min(64) {
            let index = start.advance(step).get();
            if !observed.contains(index) {
                return Ok(None);
            }
            match self
                .buffer_for_descriptor(index)
                .ok_or(RxRingError::Count)?
                .state()
            {
                ExternalRxReturnState::Attached => {}
                ExternalRxReturnState::Detached | ExternalRxReturnState::Returned => {
                    return Ok(None);
                }
            }
            if self.descriptors()[index].word0().rx_done() {
                return Ok(Some(step + 1));
            }
        }
        Ok(None)
    }

    /// Snapshot the complete units which one frozen cursor has released to
    /// software, through its LAST inclusive. A descriptor-local `RX_DONE`
    /// observation is intentionally insufficient for recycling its link
    /// word. NEXT=0 and LAST at the accepted tail release untouched
    /// intermediate descriptors which the vendor worker also walks before
    /// the terminal.
    pub fn completed_unit_frontier_through_cursor(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        cursor: RxFrozenCursor,
    ) -> Result<RxCompletedUnitFrontier, RxRingError> {
        self.validate_live_ring(ring)?;
        Ok(
            ring.completed_unit_frontier_through_cursor_with(cursor, |index| {
                self.buffer_for_descriptor(index)
                    .is_some_and(RxDmaBuffer::leading_guard_overwritten)
            }),
        )
    }

    /// Return the first complete unit at the software frontier, bounded by
    /// the LAST of one frozen cursor. This is the unit-sized counterpart used
    /// when every recycle requires its own link-release proof.
    pub fn first_completed_unit_frontier_through_cursor(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        cursor: RxFrozenCursor,
    ) -> Result<RxCompletedUnitFrontier, RxRingError> {
        self.validate_live_ring(ring)?;
        Ok(
            ring.first_completed_unit_frontier_through_cursor_with(cursor, |index| {
                self.buffer_for_descriptor(index)
                    .is_some_and(RxDmaBuffer::leading_guard_overwritten)
            }),
        )
    }

    /// Copy bytes from the first descriptor of the first complete unit without
    /// transferring descriptor ownership.
    ///
    /// This is a deliberately narrow admission-preview boundary. The ordered
    /// LAST/NEXT cursor proves that the first unit is complete, while copying
    /// values into caller-owned storage prevents a DMA buffer reference from
    /// escaping or surviving recycle. The operation neither marks the unit as
    /// observed nor changes the live ring generation.
    #[allow(
        unsafe_code,
        reason = "the frozen completed-unit frontier proves temporary CPU read ownership"
    )]
    pub fn copy_first_completed_unit_bytes_through_cursor(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        cursor: RxFrozenCursor,
        offset: usize,
        output: &mut [u8],
    ) -> Result<bool, RxRingError> {
        self.validate_live_ring(ring)?;
        let frontier = ring.first_completed_unit_frontier_through_cursor_with(cursor, |index| {
            self.buffer_for_descriptor(index)
                .is_some_and(RxDmaBuffer::leading_guard_overwritten)
        });
        if frontier.unit_count == 0 {
            return Ok(false);
        }
        let index = self
            .completion_frontier_head(ring)?
            .ok_or(RxRingError::Corrupt)?;
        let available = usize::from(ring.descriptors()[index].word0().length());
        let Some(end) = offset
            .checked_add(output.len())
            .filter(|end| *end <= available)
        else {
            return Ok(false);
        };
        let buffer = self
            .buffer_for_descriptor(index)
            .ok_or(RxRingError::Count)?;
        // SAFETY: the frozen cursor and terminal unit frontier establish that
        // DMA has released this descriptor. Only values are copied, and the
        // immutable view ends before this function returns.
        output.copy_from_slice(unsafe { &buffer.completed()[offset..end] });
        Ok(true)
    }

    /// Physical descriptor index immediately after the contiguous observed
    /// prefix retained by upper owners.
    ///
    /// This is only an index calculation; callers still need a hardware
    /// frontier proof before reading descriptor metadata or payload bytes.
    pub fn completion_frontier_head(
        &self,
        ring: &RxRingLive<'_, COUNT>,
    ) -> Result<Option<usize>, RxRingError> {
        self.validate_live_ring(ring)?;
        let observed = ring.observed_prefix_len();
        Ok((observed != COUNT).then(|| ring.recycle_start().advance(observed).get()))
    }

    /// Transfer the complete unit at the software frontier, ending at or
    /// before the LAST of `cursor` inclusive, together with its buffer read
    /// authority. Only a cursor frozen on `ring` in its current live epoch
    /// and descriptor generation takes a unit; any other takes nothing.
    pub fn take_completed_unit<'storage, 'owner, 'ring>(
        &'storage self,
        ring: &'owner mut RxRingLive<'ring, COUNT>,
        cursor: RxFrozenCursor,
    ) -> Result<
        Option<RxDmaCompletedUnit<'storage, 'owner, 'ring, COUNT, BUFFER_SIZE, STORAGE_SIZE, S>>,
        RxRingError,
    > {
        self.validate_live_ring(ring)?;
        let unit = match ring.take_completed_unit_owned(
            cursor,
            |index| {
                self.buffer_for_descriptor(index)
                    .is_some_and(RxDmaBuffer::leading_guard_overwritten)
            },
            &self.reserved_port(),
        ) {
            Ok(Some(unit)) => unit,
            Ok(None) => return Ok(None),
            Err(error) => {
                ring.require_reset();
                return Err(error);
            }
        };
        Ok(Some(RxDmaCompletedUnit {
            unit,
            storage: self,
            ring: Some(ring),
            requires_recycle: true,
            _not_send: PhantomData,
        }))
    }

    /// Claim one deferred ring-owned discard through a current LAST.
    /// A preceding upper owner or a hardware frontier short of the unit
    /// returns no credit. Append itself accepts only the affine result.
    pub fn claim_deferred_recycle<'storage, 'owner, 'ring>(
        &'storage self,
        ring: &'owner mut RxRingLive<'ring, COUNT>,
        cursor: RxFrozenCursor,
    ) -> Result<
        Option<RxDmaRecycleUnit<'storage, 'owner, 'ring, COUNT, BUFFER_SIZE, STORAGE_SIZE, S>>,
        RxRingError,
    > {
        let Some(descriptor_count) = self.first_observed_ring_unit_descriptor_count(ring)? else {
            return Ok(None);
        };
        if !ring.frozen_cursor_releases_unit(cursor, descriptor_count) {
            return Ok(None);
        }
        Ok(Some(RxDmaRecycleUnit {
            storage: self,
            ring,
            descriptor_count,
        }))
    }

    /// Check the reserved descriptor once per service pass: count a frame
    /// that landed in it before the walker reached the ring, and fail
    /// closed on any write into it after
    /// ([`RxRingLive`]'s reserved-descriptor rules).
    pub fn observe_reserved<M: RxDma>(
        &self,
        ring: &mut RxRingLive<'_, COUNT>,
        mmio: &mut M,
    ) -> Result<(), RxRingError> {
        self.validate_live_ring(ring)?;
        ring.observe_reserved_owned(mmio, &self.reserved_port())
    }

    fn validate_live_ring(&self, ring: &RxRingLive<'_, COUNT>) -> Result<(), RxRingError> {
        if ring.belongs_to_arena(&self.arena) {
            Ok(())
        } else {
            Err(RxRingError::DescriptorOwnerAddress)
        }
    }

    /// The DMA address of descriptor 0 in this arena's space.
    fn descriptor_base_address(&self) -> Result<u32, RxDmaStorageError> {
        S::address(self, self.descriptors.ring().as_ptr()).ok_or(RxDmaStorageError::AddressWidth)
    }

    /// The DMA address of `buffer`, one of this arena's buffers, as an
    /// integer for comparisons.
    fn buffer_dma_address(
        &self,
        buffer: &RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>,
    ) -> Result<u32, RxDmaStorageError> {
        S::address(self, buffer.0.get()).ok_or(RxDmaStorageError::AddressWidth)
    }

    /// The typed DMA address of `buffer`, one of this placed arena's
    /// buffers.
    ///
    /// Only [`Self::place_rotated`] calls it, with the storage the consumed
    /// `Placed` proved to lie in the DMA window.
    #[allow(unsafe_code, reason = "a buffer of the placed RX arena")]
    fn placed_buffer_address(
        &self,
        buffer: &RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>,
    ) -> Result<DmaWindowAddress, RxDmaStorageError> {
        let address = self.buffer_dma_address(buffer)?;
        // SAFETY: `address` is the DMA address of a buffer of this storage,
        // which `place_rotated` received as a consumed `Placed`: the link
        // keeps it in the DMA window and the `'static` storage stays
        // allocated for every DMA epoch. A host model's synthetic address is
        // recorded, never dereferenced.
        Ok(unsafe { crate::dma_address::placed(address) })
    }
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize, S: DmaSpace> Default
    for RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE, S>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
mod host_dma;
#[cfg(any(test, feature = "test-support"))]
pub use host_dma::HostDmaAccessError;

#[cfg(test)]
mod tests;
