//! Permanently located storage for an ESP32-S31 RX DMA ring.
//!
//! This module owns only the chip DMA memory representation. RxDescriptor count,
//! payload capacity and placement policy are selected by the board or the
//! integrating firmware and remain const-generic here.

use core::{
    marker::PhantomData,
    sync::atomic::{AtomicU8, Ordering},
};

use crate::dma_static::Placed;
use crate::layouts::{DmaWindowAddress, WifiRxDmaDescriptorControl};
use oer_memory::{ExternalRxBuffer, ExternalRxReturnState};

use crate::ieee80211::dma::{
    descriptor::RxDescriptor,
    rx_dma::{RxDescriptorAddresses, RxDma},
    rx_ring::{
        RX_BUFFER_SENTINEL, RecycleGrant, RxArenaWords, RxCompletedDescriptor, RxCompletedUnit,
        RxCompletedUnitFrontier, RxDescriptorTable, RxDmaArenaState, RxDmaBufferAddresses,
        RxFrozenCursor, RxLiveAppend, RxRingError, RxRingHalted, RxRingLive, RxRingPrepared,
        RxRingStopped, RxSegment, prepare_recycled_buffer,
    },
    space::{ChipDma, DmaSpace},
};

pub(crate) mod buffers;
use super::rx_ring::with_bound_buffers;
use buffers::BoundBuffers;
pub use buffers::RxDmaBuffer;

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
        let binding = unsafe { BoundBuffers::completed(self.buffer, self.index(), self) };
        let buffer = with_bound_buffers(binding, |ring, buffers| {
            buffers.completed_bytes(ring.read_grant())
        });
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

    pub fn append<M: RxDma>(self, mmio: &mut M) -> Result<RxLiveAppend<COUNT>, RxRingError> {
        let descriptor_count = self.descriptor_count;
        with_bound_buffers(BoundBuffers::credit(self), |ring, buffers| {
            ring.recycle_proven_unit_owned(mmio, descriptor_count, |grant| {
                buffers.prepare_for_recycle(grant)
            })
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
        // SAFETY: `RxCompletedUnit` proves that the terminal descriptor made
        // every preceding segment CPU-owned. This token retains the mutable
        // ring borrow, preventing recycle while the returned slice lives.
        let binding = unsafe { BoundBuffers::completed(self.storage, index, self) };
        with_bound_buffers(binding, |ring, buffers| {
            buffers.completed_bytes(ring.read_grant())
        })
        .ok()?
        .get(..length)
    }

    pub fn recycle<M: RxDma>(
        mut self,
        mmio: &mut M,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError> {
        let descriptor_count = self.unit.descriptor_count();
        let result = with_bound_buffers(BoundBuffers::unit(&mut self), |ring, buffers| {
            ring.recycle_completed_unit_owned(mmio, descriptor_count, |grant| {
                buffers.prepare_for_recycle(grant)
            })
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
    /// Empty storage with room for every payload and its recycle guard.
    /// The geometry is checked even when construction runs at runtime.
    ///
    /// ```compile_fail,E0080
    /// use oer_esp32s31_hal::ieee80211::dma::rx_storage::RxDmaStorage;
    /// let _ = RxDmaStorage::<1, 16, 19>::new();
    /// ```
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
        let (storage, _range, projection) = placed
            .into_dma_parts::<S>()
            .ok_or(RxDmaStorageError::AddressWidth)?;
        storage.validate_descriptor_rotation(rotation)?;
        for descriptor_index in 0..COUNT {
            let buffer_id = (descriptor_index + rotation) % COUNT;
            let address = projection
                .address(&storage.buffers[buffer_id])
                .map_err(|_| RxDmaStorageError::AddressWidth)?;
            storage.buffer_addresses.record(descriptor_index, address);
        }
        storage.bind_descriptor_rotation(rotation)?;
        let descriptors = projection
            .table(
                storage
                    .descriptors
                    .ring()
                    .iter()
                    .chain(core::iter::once(storage.descriptors.reserved()))
                    .map(|descriptor| descriptor.layout()),
            )
            .map_err(|_| RxDmaStorageError::AddressWidth)?;
        let reserved_buffer = projection
            .address(&storage.reserved_buffer)
            .map_err(|_| RxDmaStorageError::AddressWidth)?;
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

    fn prepare_ring_bound<'storage, M: RxDma>(
        &'storage self,
        mmio: &mut M,
        descriptors: RxDescriptorAddresses,
        reserved_buffer: DmaWindowAddress,
    ) -> Result<RxRingPrepared<'storage, COUNT>, RxRingError> {
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
        let prepared = with_bound_buffers(
            BoundBuffers::initial(self, descriptors, reserved_buffer),
            |ring, buffers| {
                ring.prepare_inner(
                    mmio,
                    buffer_size,
                    |grant| buffers.prepare_for_stopped_ring(grant),
                    |grant| buffers.prepare_for_stopped_ring(grant),
                )
            },
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
    pub fn prepare_halted<'storage, M: RxDma>(
        &'storage self,
        ring: RxRingHalted<'storage, COUNT>,
        mmio: &mut M,
    ) -> Result<RxRingStopped<'storage, COUNT>, (RxRingHalted<'storage, COUNT>, RxRingError)> {
        let binding = BoundBuffers::halted(self, ring)?;
        if self.any_buffer_in_state(ExternalRxReturnState::Detached) {
            return Err((binding.into_parts().1, RxRingError::OutstandingLeases));
        }
        match self.lifecycle_state() {
            RxDmaArenaState::Prepared => {}
            RxDmaArenaState::ResetRequired => {
                return Err((binding.into_parts().1, RxRingError::ResetRequired));
            }
            RxDmaArenaState::Reusable | RxDmaArenaState::Live => {
                panic!("RX halted owner invariant");
            }
        }
        let buffer_size = match u32::try_from(BUFFER_SIZE) {
            Ok(buffer_size) => buffer_size,
            Err(_) => return Err((binding.into_parts().1, RxRingError::Size)),
        };
        with_bound_buffers(binding, |ring, buffers| {
            ring.prepare_owned(
                mmio,
                buffer_size,
                |grant| buffers.prepare_for_stopped_ring(grant),
                |grant| buffers.prepare_for_stopped_ring(grant),
            )
        })
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
        let mut binding = BoundBuffers::live(self, ring)?;
        {
            let ring = binding.live_ring_mut();
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
        }
        let Some(descriptor) = with_bound_buffers(binding, |ring, buffers| {
            ring.take_completed_owned(index, &buffers)
        }) else {
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

    pub fn recycle_completed_half<M: RxDma>(
        &self,
        ring: &mut RxRingLive<'_, COUNT>,
        mmio: &mut M,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError> {
        let binding = BoundBuffers::live(self, ring)?;
        let result = with_bound_buffers(binding, |ring, buffers| {
            ring.recycle_completed_half_owned(mmio, |grant| {
                buffers.prepare_observed_for_recycle(grant)
            })
        });
        self.observe_append(result)
    }

    pub fn recycle_completed_prefix<const MAX_BATCH: usize, M: RxDma>(
        &self,
        ring: &mut RxRingLive<'_, COUNT>,
        mmio: &mut M,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError> {
        let binding = BoundBuffers::live(self, ring)?;
        let result = with_bound_buffers(binding, |ring, buffers| {
            ring.recycle_completed_prefix_owned::<MAX_BATCH, _, _>(mmio, |grant| {
                buffers.prepare_observed_for_recycle(grant)
            })
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
    pub fn recycle_released_prefix<const MAX_BATCH: usize, M: RxDma>(
        &self,
        ring: &mut RxRingLive<'_, COUNT>,
        mmio: &mut M,
    ) -> Result<Option<RxLiveAppend<COUNT>>, RxRingError> {
        let binding = BoundBuffers::live(self, ring)?;
        if MAX_BATCH == 0 {
            return Err(RxRingError::Count);
        }
        let ring = binding.live_ring();
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
        let result = with_bound_buffers(binding, |ring, buffers| {
            ring.recycle_released_terminal_prefix_owned(mmio, released, |grant| {
                buffers.prepare_observed_for_recycle(grant)
            })
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
        // SAFETY: the frozen cursor and terminal unit frontier establish that
        // DMA has released this descriptor. Only values are copied, and the
        // immutable view ends before this function returns.
        let binding = unsafe { BoundBuffers::completed(self, index, ring) };
        let bytes = with_bound_buffers(binding, |ring, buffers| {
            buffers.completed_bytes(ring.read_grant())
        })?;
        output.copy_from_slice(&bytes[offset..end]);
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
        let binding = BoundBuffers::live(self, ring)?;
        let unit = match with_bound_buffers(binding, |ring, buffers| {
            ring.take_completed_unit_owned(
                cursor,
                |index| {
                    self.buffer_for_descriptor(index)
                        .is_some_and(RxDmaBuffer::leading_guard_overwritten)
                },
                &buffers,
            )
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
        let binding = BoundBuffers::live(self, ring)?;
        with_bound_buffers(binding, |ring, buffers| {
            ring.observe_reserved_owned(mmio, &buffers)
        })
    }

    fn validate_live_ring(&self, ring: &RxRingLive<'_, COUNT>) -> Result<(), RxRingError> {
        if ring.belongs_to_arena(&self.arena) {
            Ok(())
        } else {
            Err(RxRingError::DescriptorOwnerAddress)
        }
    }

    /// The DMA address of descriptor 0 in this arena's space.
    #[cfg(any(test, feature = "test-support"))]
    fn descriptor_base_address(&self) -> Result<u32, RxDmaStorageError> {
        S::address(self, self.descriptors.ring().as_ptr()).ok_or(RxDmaStorageError::AddressWidth)
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
pub use buffers::host_dma::HostDmaAccessError;

#[cfg(test)]
mod tests;
