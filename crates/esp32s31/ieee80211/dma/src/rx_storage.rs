//! Permanently located storage for an ESP32-S31 RX DMA ring.
//!
//! This module owns only the chip DMA memory representation. Descriptor count,
//! payload capacity and placement policy are selected by the board or the
//! integrating firmware and remain const-generic here.

use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    ptr::NonNull,
    sync::atomic::{AtomicU8, Ordering},
};

use oer_memory::ExternalRxBuffer;

use crate::{
    descriptor::Descriptor,
    rx_dma::RxDma,
    rx_ring::{
        RX_BUFFER_SENTINEL, RxArenaWords, RxCompletedDescriptor, RxCompletedUnit,
        RxCompletedUnitFrontier, RxDmaArenaState, RxDmaBufferAddresses, RxFrozenCursor,
        RxLiveAppend, RxRingError, RxRingHalted, RxRingLive, RxRingStopped, RxSegment,
        prepare_recycled_buffer,
    },
    space::{ChipDma, DmaSpace},
};

/// One aligned DMA-visible buffer with room for the hardware recycle guard.
#[allow(
    unsafe_code,
    reason = "a descriptor's zero words are the empty descriptor"
)]
// REVIEWED-LAYOUT: vcell 0.1.3
// SAFETY: `vcell::VolatileCell<u32>` is `repr(transparent)` over
// `UnsafeCell<u32>`, so a descriptor's zero bytes are three zero words: the
// empty, software-owned descriptor `Descriptor::new` builds. (`descriptor.rs`
// forbids `unsafe`, and neither `vcell` nor `bytemuck` is ours.)
unsafe impl bytemuck::Zeroable for crate::descriptor::Descriptor {}

#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
pub struct RxDmaBuffer<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>(
    UnsafeCell<[u8; STORAGE_SIZE]>,
    AtomicU8,
);

const RX_BUFFER_RING: u8 = 0;
const RX_BUFFER_DETACHED: u8 = 1;
const RX_BUFFER_RELEASED: u8 = 2;

impl<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE> {
    const fn new() -> Self {
        assert!(STORAGE_SIZE >= BUFFER_SIZE + 4);
        Self(
            UnsafeCell::new([0; STORAGE_SIZE]),
            AtomicU8::new(RX_BUFFER_RING),
        )
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
        match self.1.load(Ordering::Acquire) {
            RX_BUFFER_RING => {}
            RX_BUFFER_DETACHED | RX_BUFFER_RELEASED => return Err(RxRingError::Busy),
            _ => return Err(RxRingError::Corrupt),
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
    /// durable `Released` state. Normalize that state in the same transaction;
    /// otherwise the descriptor becomes DMA-visible while a later detach sees
    /// the stale release and rejects a real completion as `Busy`.
    #[allow(
        unsafe_code,
        reason = "the observed live-ring prefix excludes DMA and detached upper ownership"
    )]
    unsafe fn prepare_observed_for_recycle(&self) -> Result<(), RxRingError> {
        match self.1.load(Ordering::Acquire) {
            RX_BUFFER_RING => {}
            RX_BUFFER_RELEASED => {
                self.1
                    .compare_exchange(
                        RX_BUFFER_RELEASED,
                        RX_BUFFER_RING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .map_err(|_| RxRingError::Busy)?;
            }
            RX_BUFFER_DETACHED => return Err(RxRingError::Busy),
            _ => return Err(RxRingError::Corrupt),
        }
        // SAFETY: the state check/transition excludes every external owner.
        unsafe { self.prepare_for_recycle() }
    }

    fn detach_with(
        &self,
        length: usize,
        index: usize,
        owner: NonNull<()>,
        release: unsafe fn(NonNull<()>, usize),
    ) -> Result<ExternalRxBuffer, RxRingError> {
        if length == 0 || length > BUFFER_SIZE {
            return Err(RxRingError::Size);
        }
        self.1
            .compare_exchange(
                RX_BUFFER_RING,
                RX_BUFFER_DETACHED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| RxRingError::Busy)?;
        let pointer = NonNull::new(self.0.get().cast::<u8>()).expect("RX DMA buffer is non-null");
        #[allow(
            unsafe_code,
            reason = "completed descriptor detaches stable DMA buffer ownership"
        )]
        // SAFETY: the completed-unit owner proved that DMA released this
        // buffer. The buffer is part of stable arena storage and the state
        // transition prevents recycle until the callback marks it released.
        Ok(unsafe { ExternalRxBuffer::new(pointer, length, BUFFER_SIZE, owner, index, release) })
    }

    fn is_released(&self) -> bool {
        self.1.load(Ordering::Acquire) == RX_BUFFER_RELEASED
    }

    fn state(&self) -> u8 {
        self.1.load(Ordering::Acquire)
    }

    #[allow(
        unsafe_code,
        reason = "stopped DMA owner may normalize a returned detached buffer"
    )]
    unsafe fn prepare_for_stopped_ring(&self) -> Result<(), RxRingError> {
        match self.1.load(Ordering::Acquire) {
            RX_BUFFER_RING => {}
            RX_BUFFER_RELEASED => {
                self.1
                    .compare_exchange(
                        RX_BUFFER_RELEASED,
                        RX_BUFFER_RING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .map_err(|_| RxRingError::Busy)?;
            }
            RX_BUFFER_DETACHED => return Err(RxRingError::Busy),
            _ => return Err(RxRingError::Corrupt),
        }
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

#[allow(
    unsafe_code,
    reason = "type-erased external lease returns to its concrete DMA buffer"
)]
unsafe fn release_detached_buffer<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>(
    owner: NonNull<()>,
    _index: usize,
) {
    // SAFETY: `RxDmaStorage::detach_buffer` installed this callback with a pointer to
    // the same concrete buffer allocation, which remains static for the DMA
    // epoch.
    let buffer = unsafe {
        owner
            .cast::<RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>>()
            .as_ref()
    };
    assert_eq!(
        buffer.1.compare_exchange(
            RX_BUFFER_DETACHED,
            RX_BUFFER_RELEASED,
            Ordering::Release,
            Ordering::Acquire,
        ),
        Ok(RX_BUFFER_DETACHED),
        "detached RX DMA buffer must be released exactly once"
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxDmaStorageError {
    AddressWidth,
    Count,
}

/// First immutable DMA-address binding that no longer matches its arena.
///
/// This is fault evidence only. It exposes neither descriptor mutation nor a
/// way to repair a live ring after adjacent memory corruption.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBufferAddressMismatch {
    pub index: usize,
    pub expected: u32,
    pub observed: u32,
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
    ring: &'owner mut RxRingLive<'ring, COUNT>,
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
    descriptor_word0: u32,
    length: usize,
}

impl RxDmaDetachedUnit {
    pub const fn descriptor_address(&self) -> u32 {
        self.descriptor_address
    }

    pub const fn descriptor_word0(&self) -> u32 {
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

    /// Detach one complete hardware buffer for zero-copy upper ownership.
    ///
    /// The descriptor remains observed and CPU-owned in the ring. Dropping
    /// the returned buffer marks it released; only a later ring-owner service
    /// may rearm the descriptor.
    pub fn detach_single(mut self) -> Result<RxDmaDetachedUnit, RxRingError>
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
        let buffer = self
            .storage
            .detach_buffer(self.unit.total_length(), buffer_id)?;
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
            self.ring.require_reset();
        }
    }
}

/// Descriptor and buffer arena whose address remains stable for a DMA epoch.
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
    descriptors: [Descriptor; COUNT],
    buffers: [RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>; COUNT],
    /// Per descriptor, the distance to its bound buffer: descriptor `i` uses
    /// buffer `(i + offset) % COUNT`. Zero is the identity binding, so the
    /// arena's zero bytes bind every descriptor to its own buffer.
    descriptor_buffer_offsets: [AtomicU8; COUNT],
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

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize, S: DmaSpace>
    RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE, S>
{
    pub const fn new() -> Self {
        Self {
            descriptors: [const { Descriptor::new() }; COUNT],
            buffers: [const { RxDmaBuffer::new() }; COUNT],
            descriptor_buffer_offsets: [const { AtomicU8::new(0) }; COUNT],
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
        let detached = buffer.detach_with(
            length,
            index,
            NonNull::from(buffer).cast(),
            release_detached_buffer::<BUFFER_SIZE, STORAGE_SIZE>,
        );
        let detached = detached?;
        Ok(detached)
    }

    fn observe_append(
        &self,
        result: Result<Option<RxLiveAppend<COUNT>>, RxRingError>,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError> {
        result
    }

    /// Sticky lifecycle state stored with the static arena rather than in its
    /// movable ring token.
    pub fn lifecycle_state(&self) -> RxDmaArenaState {
        RxDmaArenaState::load(self.arena.lifecycle())
    }

    pub const fn descriptors(&self) -> &[Descriptor; COUNT] {
        &self.descriptors
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

    pub fn dma_layout(
        &self,
        buffer_addresses: &mut RxDmaBufferAddresses<COUNT>,
    ) -> Result<u32, RxDmaStorageError> {
        self.dma_layout_rotated(buffer_addresses, 0)
    }

    /// Bind each physical descriptor to a cyclically shifted arena buffer.
    ///
    /// Rotation zero is the production identity layout. A non-zero rotation
    /// is a target proof that every CPU ownership path follows the explicit
    /// descriptor-to-buffer binding instead of indexing the buffer arena by
    /// descriptor number. The binding is frozen before the first ring owner
    /// exists and remains immutable for that complete DMA lifecycle.
    #[allow(
        unsafe_code,
        reason = "Reusable lifecycle is the exclusive pre-ring binding boundary"
    )]
    pub fn dma_layout_rotated(
        &self,
        buffer_addresses: &mut RxDmaBufferAddresses<COUNT>,
        rotation: usize,
    ) -> Result<u32, RxDmaStorageError> {
        self.validate_descriptor_rotation(rotation)?;
        for (descriptor_index, address) in buffer_addresses.iter_mut().enumerate() {
            let buffer_id = (descriptor_index + rotation) % COUNT;
            *address = self.buffer_dma_address(&self.buffers[buffer_id])?;
        }
        self.bind_descriptor_rotation(rotation)?;
        self.descriptor_base_address()
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

    fn any_buffer_in_state(&self, state: u8) -> bool {
        self.buffers.iter().any(|buffer| buffer.state() == state)
    }

    pub fn detached_buffer_count(&self) -> usize {
        self.buffers
            .iter()
            .filter(|buffer| buffer.state() == RX_BUFFER_DETACHED)
            .count()
    }

    fn buffer_by_id(&self, buffer_id: usize) -> Option<&RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>> {
        self.buffers.get(buffer_id)
    }

    /// Prepare the first stopped ring on a hardware target.
    ///
    /// The `'static` receiver is essential: safe code may forget a live ring,
    /// so DMA-visible descriptors and buffers must remain allocated even when
    /// the typestate owner is lost. Borrowing the arena for `'static` also
    /// prevents later safe mutable pre-publication access.
    ///
    /// Temporary storage cannot authorize a DMA lifecycle, including in a model:
    /// ```compile_fail,E0597
    /// use oer_esp32s31_ieee80211_dma::{rx_dma::RxDma, rx_storage::RxDmaStorage};
    /// fn temporary(hardware: &mut impl RxDma) {
    ///     let storage = RxDmaStorage::<2, 16, 20>::new();
    ///     let _ = storage.prepare_ring(hardware, 0x2f001000, &[0x2f002000, 0x2f002100]);
    /// }
    /// ```
    /// A static arena can return an owner borrowing the address table:
    /// ```
    /// use oer_esp32s31_ieee80211_dma::{
    ///     rx_dma::RxDma, rx_ring::{RxRingError, RxRingStopped}, rx_storage::RxDmaStorage,
    /// };
    /// fn prepare<'a>(storage: &'static RxDmaStorage<2, 16, 20>,
    ///     hardware: &mut impl RxDma, addresses: &'a [u32; 2])
    ///     -> Result<RxRingStopped<'a, 2>, RxRingError> {
    ///     storage.prepare_ring(hardware, 0x2f001000, addresses)
    /// }
    /// ```
    pub fn prepare_ring<'addresses, M: RxDma>(
        &'static self,
        mmio: &mut M,
        descriptor_base: u32,
        buffer_addresses: &'addresses RxDmaBufferAddresses<COUNT>,
    ) -> Result<RxRingStopped<'addresses, COUNT>, RxRingError> {
        self.prepare_ring_bound(mmio, descriptor_base, buffer_addresses)
    }

    #[allow(
        unsafe_code,
        reason = "stopped ring exclusively owns initial buffer rearm"
    )]
    fn prepare_ring_bound<'storage, M: RxDma>(
        &'storage self,
        mmio: &mut M,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
    ) -> Result<RxRingStopped<'storage, COUNT>, RxRingError> {
        self.validate_descriptor_base(descriptor_base)?;
        self.validate_buffer_addresses(buffer_addresses)?;
        match self.lifecycle_state() {
            RxDmaArenaState::Reusable => {}
            RxDmaArenaState::Prepared | RxDmaArenaState::Live => {
                return Err(RxRingError::Busy);
            }
            RxDmaArenaState::ResetRequired => return Err(RxRingError::ResetRequired),
        }
        let buffer_size = u32::try_from(BUFFER_SIZE).map_err(|_| RxRingError::Size)?;
        // `prepare_ring` takes a shared static arena reference, so Rust
        // borrowing alone cannot prevent two root capabilities. Claim the
        // stopped/halted owner graph before mutating any descriptor.
        if !RxDmaArenaState::claim_prepared(self.arena.lifecycle()) {
            return Err(RxRingError::Busy);
        }
        let prepared = RxRingStopped::prepare_inner(
            mmio,
            &self.descriptors,
            descriptor_base,
            buffer_addresses,
            buffer_size,
            |index| {
                // SAFETY: stopped-ring preparation owns every descriptor and
                // the validated address table binds `index` to this arena.
                unsafe { self.prepare_stopped_buffer(index) }
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
        if self.any_buffer_in_state(RX_BUFFER_DETACHED) {
            return Err((ring, RxRingError::Busy));
        }
        match self.lifecycle_state() {
            RxDmaArenaState::Prepared => {}
            RxDmaArenaState::ResetRequired => {
                return Err((ring, RxRingError::ResetRequired));
            }
            RxDmaArenaState::Reusable | RxDmaArenaState::Live => {
                return Err((ring, RxRingError::Busy));
            }
        }
        if let Err(error) = self.validate_ring_layout(
            ring.descriptor_base(),
            ring.descriptors(),
            ring.buffer_addresses(),
        ) {
            return Err((ring, error));
        }
        let buffer_size = match u32::try_from(BUFFER_SIZE) {
            Ok(buffer_size) => buffer_size,
            Err(_) => return Err((ring, RxRingError::Size)),
        };
        ring.prepare_owned(mmio, buffer_size, |index| {
            // SAFETY: the halted owner proves that DMA is stopped, and the
            // validated layout binds the descriptor to this exact buffer.
            unsafe { self.prepare_stopped_buffer(index) }
        })
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
    pub fn take_completed<'owner, 'ring>(
        &'owner self,
        ring: &'owner mut RxRingLive<'ring, COUNT>,
        index: usize,
    ) -> Result<
        Option<RxDmaCompletedDescriptor<'owner, 'ring, COUNT, BUFFER_SIZE, STORAGE_SIZE>>,
        RxRingError,
    > {
        self.validate_live_ring(ring)?;
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
        let Some(descriptor) = ring.take_completed_owned(index) else {
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
                || !crate::descriptor::rx_done(self.descriptors[index].word0())
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
    /// returned network buffer (`Released`) and a live upper lease
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
                RX_BUFFER_RING => {}
                RX_BUFFER_DETACHED | RX_BUFFER_RELEASED => return Ok(None),
                _ => return Err(RxRingError::Corrupt),
            }
            if crate::descriptor::rx_done(self.descriptors[index].word0()) {
                return Ok(Some(step + 1));
            }
        }
        Ok(None)
    }

    pub fn completed_unit_frontier(
        &self,
        ring: &RxRingLive<'_, COUNT>,
    ) -> Result<RxCompletedUnitFrontier, RxRingError> {
        self.validate_live_ring(ring)?;
        Ok(ring.completed_unit_frontier_with(|index| {
            self.buffer_for_descriptor(index)
                .is_some_and(RxDmaBuffer::leading_guard_overwritten)
        }))
    }

    /// Return only complete units which the hardware last-descriptor frontier
    /// has released to software. A descriptor-local `RX_DONE` observation is
    /// intentionally insufficient for recycling its link word.
    pub fn completed_unit_frontier_through(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        last_descriptor_low: u32,
    ) -> Result<RxCompletedUnitFrontier, RxRingError> {
        self.validate_live_ring(ring)?;
        Ok(
            ring.completed_unit_frontier_through_with(last_descriptor_low, |index| {
                self.buffer_for_descriptor(index)
                    .is_some_and(RxDmaBuffer::leading_guard_overwritten)
            }),
        )
    }

    /// LAST-bounded frontier with the matching ordered NEXT observation.
    /// NEXT=0 and LAST at the accepted tail release untouched intermediate
    /// descriptors which the vendor worker also walks before the terminal.
    pub fn completed_unit_frontier_through_cursor(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        last_descriptor_low: u32,
        next_descriptor_low: u32,
    ) -> Result<RxCompletedUnitFrontier, RxRingError> {
        self.validate_live_ring(ring)?;
        Ok(ring.completed_unit_frontier_through_cursor_with(
            last_descriptor_low,
            next_descriptor_low,
            |index| {
                self.buffer_for_descriptor(index)
                    .is_some_and(RxDmaBuffer::leading_guard_overwritten)
            },
        ))
    }

    /// Return the first complete unit at the software frontier, bounded by
    /// the hardware LAST snapshot. This is the unit-sized counterpart used
    /// when every recycle requires its own link-release proof.
    pub fn first_completed_unit_frontier_through(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        last_descriptor_low: u32,
    ) -> Result<RxCompletedUnitFrontier, RxRingError> {
        self.validate_live_ring(ring)?;
        Ok(
            ring.first_completed_unit_frontier_through_with(last_descriptor_low, |index| {
                self.buffer_for_descriptor(index)
                    .is_some_and(RxDmaBuffer::leading_guard_overwritten)
            }),
        )
    }

    pub fn first_completed_unit_frontier_through_cursor(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        last_descriptor_low: u32,
        next_descriptor_low: u32,
    ) -> Result<RxCompletedUnitFrontier, RxRingError> {
        self.validate_live_ring(ring)?;
        Ok(ring.first_completed_unit_frontier_through_cursor_with(
            last_descriptor_low,
            next_descriptor_low,
            |index| {
                self.buffer_for_descriptor(index)
                    .is_some_and(RxDmaBuffer::leading_guard_overwritten)
            },
        ))
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
        last_descriptor_low: u32,
        next_descriptor_low: u32,
        offset: usize,
        output: &mut [u8],
    ) -> Result<bool, RxRingError> {
        self.validate_live_ring(ring)?;
        let frontier = ring.first_completed_unit_frontier_through_cursor_with(
            last_descriptor_low,
            next_descriptor_low,
            |index| {
                self.buffer_for_descriptor(index)
                    .is_some_and(RxDmaBuffer::leading_guard_overwritten)
            },
        );
        if frontier.unit_count == 0 {
            return Ok(false);
        }
        let index = self
            .completion_frontier_head(ring)?
            .ok_or(RxRingError::Corrupt)?;
        let available = crate::descriptor::length(ring.descriptors()[index].word0()) as usize;
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

    pub fn take_completed_unit<'storage, 'owner, 'ring>(
        &'storage self,
        ring: &'owner mut RxRingLive<'ring, COUNT>,
        descriptor_limit: usize,
    ) -> Result<
        Option<RxDmaCompletedUnit<'storage, 'owner, 'ring, COUNT, BUFFER_SIZE, STORAGE_SIZE, S>>,
        RxRingError,
    > {
        self.validate_live_ring(ring)?;
        let unit = match ring.take_completed_unit_owned(descriptor_limit, |index| {
            self.buffer_for_descriptor(index)
                .is_some_and(RxDmaBuffer::leading_guard_overwritten)
        }) {
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
            ring,
            requires_recycle: true,
            _not_send: PhantomData,
        }))
    }

    /// Return one copied complete unit through the vendor's frozen-LAST
    /// ownership proof.
    #[allow(
        unsafe_code,
        reason = "the frozen vendor LAST proves exclusive unit-buffer rearm"
    )]
    pub fn recycle_completed_unit_through_frozen_last<M: RxDma>(
        &self,
        ring: &mut RxRingLive<'_, COUNT>,
        mmio: &mut M,
        cursor: RxFrozenCursor,
        descriptor_count: usize,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError> {
        self.validate_live_ring(ring)?;
        let result = ring.recycle_completed_unit_through_frozen_last_owned(
            mmio,
            cursor,
            descriptor_count,
            |index| {
                // SAFETY: the ring limits mutation to the observed complete
                // unit ending at or before the generation-bound LAST.
                unsafe {
                    self.buffer_for_descriptor(index)
                        .ok_or(RxRingError::Count)?
                        .prepare_for_recycle()
                }
            },
        );
        self.observe_append(result)
    }

    fn validate_live_ring(&self, ring: &RxRingLive<'_, COUNT>) -> Result<(), RxRingError> {
        if ring.belongs_to_arena(&self.arena) {
            return Ok(());
        }
        self.validate_ring_layout(
            ring.descriptor_base(),
            ring.descriptors(),
            ring.buffer_addresses(),
        )
    }

    fn validate_ring_layout(
        &self,
        descriptor_base: u32,
        descriptors: &[Descriptor; COUNT],
        buffer_addresses: &RxDmaBufferAddresses<COUNT>,
    ) -> Result<(), RxRingError> {
        if !core::ptr::eq(descriptors, &self.descriptors) {
            return Err(RxRingError::DescriptorOwnerAddress);
        }
        self.validate_descriptor_base(descriptor_base)?;
        self.validate_buffer_addresses(buffer_addresses)
    }

    /// The DMA address of descriptor 0 in this arena's space.
    fn descriptor_base_address(&self) -> Result<u32, RxDmaStorageError> {
        S::address(self, self.descriptors.as_ptr()).ok_or(RxDmaStorageError::AddressWidth)
    }

    /// The DMA address of `buffer`, one of this arena's buffers.
    fn buffer_dma_address(
        &self,
        buffer: &RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>,
    ) -> Result<u32, RxDmaStorageError> {
        S::address(self, buffer.0.get()).ok_or(RxDmaStorageError::AddressWidth)
    }

    fn validate_descriptor_base(&self, descriptor_base: u32) -> Result<(), RxRingError> {
        if self
            .descriptor_base_address()
            .map_err(|_| RxRingError::Address)?
            != descriptor_base
        {
            return Err(RxRingError::DescriptorBaseAddress);
        }
        Ok(())
    }

    fn validate_buffer_addresses(
        &self,
        buffer_addresses: &RxDmaBufferAddresses<COUNT>,
    ) -> Result<(), RxRingError> {
        for (index, &address) in buffer_addresses.iter().enumerate() {
            let buffer = self
                .buffer_for_descriptor(index)
                .ok_or(RxRingError::Count)?;
            if self
                .buffer_dma_address(buffer)
                .map_err(|_| RxRingError::Address)?
                != address
            {
                return Err(if index + 1 == COUNT {
                    RxRingError::TailBufferAddress
                } else {
                    RxRingError::BufferAddress
                });
            }
        }
        Ok(())
    }

    pub fn first_buffer_address_mismatch(
        &self,
        buffer_addresses: &RxDmaBufferAddresses<COUNT>,
    ) -> Option<RxBufferAddressMismatch> {
        for (index, &observed) in buffer_addresses.iter().enumerate() {
            let buffer = self.buffer_for_descriptor(index)?;
            let expected = self.buffer_dma_address(buffer).ok()?;
            if expected != observed {
                return Some(RxBufferAddressMismatch {
                    index,
                    expected,
                    observed,
                });
            }
        }
        None
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
