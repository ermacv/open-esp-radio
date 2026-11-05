//! Pinned descriptor and backing ownership for one TX A-MPDU.
//!
//! Internally buffered aggregates retain their static arena directly.
//! External zero-copy aggregates receive prepare/start authority only through
//! an owner which also retains every referenced [`StableDmaBacking`](oer_portable_memory::StableDmaBacking) lease.

use crate::ieee80211::dma::space::DmaSpace;
use crate::memory::dma_address::{PlacedRegion, PlacedSlots, PlacedTable};
use core::{
    marker::{PhantomData, PhantomPinned},
    mem,
    mem::ManuallyDrop,
    pin::Pin,
};

use super::tx_storage::{CommitOwner, CommitTxDma};
use crate::memory::dma_static::{Placed, PlacedBacking};
use crate::types::MacTxQueueDetached;
use oer_portable_memory::{HardwareOwnedTxDma, PreparedTxDma, StableDmaRange};
use pin_project::pin_project;

use crate::layouts::{DmaRef, DmaWindowAddress, WifiTxDmaDescriptor, WifiTxDmaDescriptorControl};

use crate::ieee80211::dma::descriptor::{DESCRIPTOR_BYTES, TxDescriptor, tx_owned_word};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, bytemuck::Zeroable)]
#[repr(u8)]
pub enum AmpduDmaState {
    #[default]
    Free,
    Reserved,
    HardwareOwned,
    Completed,
    Detached,
    ResetRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduDmaStorageError {
    Address,
    Count,
    GenerationExhausted,
    InvalidLength,
    StaleBacking,
    State,
    /// A [`RetainedAmpduDma`] no longer holds its arenas. Only
    /// [`RetainedAmpduDma::try_into_parts`], which consumes the owner, takes
    /// them, so no method of a live owner meets it.
    Retired,
}

/// One internal A-MPDU descriptor entry, excluding its derived addresses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduInternalDescriptor {
    pub buffer_capacity: u32,
    pub transfer_length: u32,
}

#[repr(C, align(16))]
#[derive(bytemuck::Zeroable)]
struct AmpduDmaBuffer<const BUFFER_SIZE: usize>([u8; BUFFER_SIZE]);

/// Final static allocation for an internal-buffer A-MPDU arena.
///
/// Its layout is fixed: the descriptors, then the 16-byte aligned buffers.
#[pin_project]
#[derive(bytemuck::Zeroable)]
#[repr(C)]
pub struct AmpduDmaStorage<const SLOTS: usize, const BUFFER_SIZE: usize> {
    #[pin]
    descriptors: [TxDescriptor; SLOTS],
    #[pin]
    buffers: [AmpduDmaBuffer<BUFFER_SIZE>; SLOTS],
    state: AmpduDmaState,
    lease_generation: u64,
    /// Exact descriptor prefix published by the current or most recently
    /// detached transaction. It belongs to the pinned arena rather than the
    /// movable capability so lifecycle futures do not carry DMA bookkeeping.
    published_count: usize,
    #[pin]
    _pin: PhantomPinned,
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> AmpduDmaStorage<SLOTS, BUFFER_SIZE> {
    pub const fn new() -> Self {
        Self {
            descriptors: [const { TxDescriptor::new() }; SLOTS],
            buffers: [const { AmpduDmaBuffer([0; BUFFER_SIZE]) }; SLOTS],
            state: AmpduDmaState::Free,
            lease_generation: 0,
            published_count: 0,
            _pin: PhantomPinned,
        }
    }

    /// Bind the placed storage at its DMA addresses in space `S`, once. The
    /// link keeps the placed storage in the DMA window, so its addresses
    /// need no window check.
    pub fn pin<S: DmaSpace>(
        placed: Placed<Self>,
    ) -> Result<PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>, AmpduDmaStorageError> {
        let (storage, range, projection) = placed
            .into_dma_parts::<S>()
            .ok_or(AmpduDmaStorageError::Address)?;
        let descriptor_count = u8::try_from(SLOTS).map_err(|_| AmpduDmaStorageError::Count)?;
        if descriptor_count == 0 {
            return Err(AmpduDmaStorageError::Count);
        }
        let descriptors = projection
            .table::<WifiTxDmaDescriptor>(
                storage
                    .descriptors
                    .iter()
                    .map(|descriptor| descriptor.layout()),
            )
            .map_err(|_| AmpduDmaStorageError::Address)?;
        let descriptor_len = u32::try_from(core::mem::size_of_val(&storage.descriptors))
            .map_err(|_| AmpduDmaStorageError::Address)?;
        let descriptor_range = range
            .subrange(descriptors.base(), descriptor_len)
            .ok_or(AmpduDmaStorageError::Address)?;
        let buffer_capacity =
            u32::try_from(BUFFER_SIZE).map_err(|_| AmpduDmaStorageError::Address)?;
        let (buffer_range, buffers) = if BUFFER_SIZE == 0 {
            (None, None)
        } else {
            let buffers = projection
                .slots(&storage.buffers)
                .map_err(|_| AmpduDmaStorageError::Address)?;
            let buffer_len = u32::try_from(core::mem::size_of_val(&storage.buffers))
                .map_err(|_| AmpduDmaStorageError::Address)?;
            let buffer_range = range
                .subrange(buffers.base(), buffer_len)
                .ok_or(AmpduDmaStorageError::Address)?;
            (Some(buffer_range), Some(buffers))
        };

        Ok(PinnedAmpduDmaStorage {
            storage: Pin::static_mut(storage),
            binding: AmpduDmaBinding {
                descriptors,
                descriptor_count,
                descriptor_range,
                buffers,
                buffer_capacity,
                buffer_range,
            },
        })
    }
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> Default for AmpduDmaStorage<SLOTS, BUFFER_SIZE> {
    fn default() -> Self {
        Self::new()
    }
}

/// Private-range authority for one statically retained aggregate arena.
pub struct AmpduDmaBinding {
    descriptors: PlacedTable<WifiTxDmaDescriptor>,
    descriptor_count: u8,
    descriptor_range: StableDmaRange<'static>,
    /// The internal buffers, `None` for an arena without them.
    buffers: Option<PlacedSlots>,
    buffer_capacity: u32,
    buffer_range: Option<StableDmaRange<'static>>,
}

impl AmpduDmaBinding {
    pub const fn descriptor_head(&self) -> u32 {
        self.descriptors.base()
    }

    pub const fn descriptor_count(&self) -> u8 {
        self.descriptor_count
    }

    pub fn descriptor_address(&self, index: usize) -> Option<u32> {
        self.descriptor(index).map(DmaRef::bits)
    }

    /// Descriptor `index` of the arena, inside its private range.
    fn descriptor(&self, index: usize) -> Option<DmaRef<WifiTxDmaDescriptor>> {
        (index < usize::from(self.descriptor_count))
            .then(|| self.descriptors.element(index))
            .flatten()
            .filter(|element| {
                self.descriptor_range
                    .contains(element.bits(), DESCRIPTOR_BYTES)
            })
    }

    pub fn internal_buffer_address(&self, index: usize) -> Option<u32> {
        self.internal_buffer(index).map(DmaWindowAddress::get)
    }

    /// Internal buffer `index` of the arena, inside its private range.
    fn internal_buffer(&self, index: usize) -> Option<DmaWindowAddress> {
        let range = self.buffer_range.as_ref()?;
        (index < usize::from(self.descriptor_count))
            .then(|| self.buffers?.slot(index))
            .flatten()
            .filter(|address| range.contains(address.get(), self.buffer_capacity))
    }

    pub const fn internal_buffer_capacity(&self) -> u32 {
        self.buffer_capacity
    }
}

pub struct AmpduDmaStart<'owner> {
    binding: &'owner AmpduDmaBinding,
}

#[allow(
    unsafe_code,
    reason = "start token exists only after aggregate hardware ownership is recorded"
)]
// SAFETY: `commit` records `HardwareOwned` for the whole aggregate before
// creating this token and only lends it to the start closure; the owner keeps
// every descriptor and buffer until completion or reset, and the private
// field prevents forging the token.
unsafe impl HardwareOwnedTxDma<DmaRef<WifiTxDmaDescriptor>> for AmpduDmaStart<'_> {
    fn descriptor_head(&self) -> DmaRef<WifiTxDmaDescriptor> {
        self.binding.descriptors.first()
    }
}

/// Movable capability for one permanently located aggregate DMA arena.
///
/// Drop is intentionally not a queue operation. Hardware-owned or quarantined
/// backing remains in its static state until explicit detach/reset, and any
/// external leases retained by [`RetainedAmpduDma`] are forgotten rather than
/// returned while the peripheral may still reference them.
pub struct PinnedAmpduDmaStorage<const SLOTS: usize, const BUFFER_SIZE: usize> {
    storage: Pin<&'static mut AmpduDmaStorage<SLOTS, BUFFER_SIZE>>,
    binding: AmpduDmaBinding,
}

/// Only the software phase lends mutable internal backing.
pub enum AmpduDmaAccess<'owner, const SLOTS: usize, const BUFFER_SIZE: usize> {
    Writable(AmpduDmaWritable<'owner, SLOTS, BUFFER_SIZE>),
    Unavailable(AmpduDmaState),
}

pub struct AmpduDmaWritable<'owner, const SLOTS: usize, const BUFFER_SIZE: usize> {
    owner: &'owner mut PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>,
}

impl<'owner, const SLOTS: usize, const BUFFER_SIZE: usize>
    AmpduDmaWritable<'owner, SLOTS, BUFFER_SIZE>
{
    pub fn buffer_mut(
        &mut self,
        index: usize,
    ) -> Result<&mut [u8; BUFFER_SIZE], AmpduDmaStorageError> {
        self.owner.writable_buffer_mut(index)
    }
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE> {
    pub fn state(&self) -> AmpduDmaState {
        self.storage.as_ref().get_ref().state
    }

    pub const fn binding(&self) -> &AmpduDmaBinding {
        &self.binding
    }

    pub fn begin(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.transition(AmpduDmaState::Free, AmpduDmaState::Reserved)
    }

    /// Lend buffer-edit authority only while software owns the allocation.
    pub fn access(&mut self) -> AmpduDmaAccess<'_, SLOTS, BUFFER_SIZE> {
        match self.state() {
            AmpduDmaState::Free | AmpduDmaState::Reserved => {
                AmpduDmaAccess::Writable(AmpduDmaWritable { owner: self })
            }
            state => AmpduDmaAccess::Unavailable(state),
        }
    }

    fn writable_buffer_mut(
        &mut self,
        index: usize,
    ) -> Result<&mut [u8; BUFFER_SIZE], AmpduDmaStorageError> {
        self.storage
            .as_mut()
            .project()
            .buffers
            .get_mut()
            .get_mut(index)
            .map(|buffer| &mut buffer.0)
            .ok_or(AmpduDmaStorageError::Count)
    }

    /// Publish a complete chain backed only by this owner's internal buffers.
    pub fn publish_internal_chain(
        &mut self,
        entries: &[AmpduInternalDescriptor],
    ) -> Result<AmpduDmaPublication<'_, SLOTS, BUFFER_SIZE>, AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Reserved {
            return Err(AmpduDmaStorageError::State);
        }
        if entries.is_empty() || entries.len() > SLOTS {
            return Err(AmpduDmaStorageError::Count);
        }

        // Validate and derive the entire image before touching descriptor
        // memory. In particular, a backing arena may be larger than the
        // descriptor's 14-bit size field; discovering that on a later entry
        // must not leave an earlier descriptor published.
        let mut words = [WifiTxDmaDescriptorControl::ZERO; SLOTS];
        let mut buffer_addresses = [None; SLOTS];
        let mut descriptor_addresses = [None; SLOTS];
        for (index, entry) in entries.iter().enumerate() {
            if entry.buffer_capacity == 0
                || entry.buffer_capacity > self.binding.buffer_capacity
                || entry.transfer_length == 0
                || entry.transfer_length > entry.buffer_capacity
            {
                return Err(AmpduDmaStorageError::InvalidLength);
            }
            buffer_addresses[index] = Some(
                self.binding
                    .internal_buffer(index)
                    .ok_or(AmpduDmaStorageError::Address)?,
            );
            descriptor_addresses[index] = Some(
                self.binding
                    .descriptor(index)
                    .ok_or(AmpduDmaStorageError::Address)?,
            );
            words[index] = tx_owned_word(entry.buffer_capacity, entry.transfer_length)
                .ok_or(AmpduDmaStorageError::InvalidLength)?;
        }

        for index in 0..entries.len() {
            let next = if index + 1 == entries.len() {
                None
            } else {
                descriptor_addresses[index + 1]
            };
            let word0 = words[index].with_tx_last(next.is_none());
            let buffer = buffer_addresses[index].ok_or(AmpduDmaStorageError::Address)?;
            self.storage.as_ref().get_ref().descriptors[index].publish_owned(word0, buffer, next);
        }
        *self.storage.as_mut().project().published_count = entries.len();

        Ok(AmpduDmaPublication { owner: self })
    }

    pub fn descriptor_word0(&self, index: usize) -> Option<WifiTxDmaDescriptorControl> {
        self.storage
            .as_ref()
            .get_ref()
            .descriptors
            .get(index)
            .map(TxDescriptor::word0)
    }

    pub fn descriptor_next_address(&self, index: usize) -> Option<u32> {
        self.storage
            .as_ref()
            .get_ref()
            .descriptors
            .get(index)
            .map(TxDescriptor::next_address)
    }

    pub fn descriptor_buffer_address(&self, index: usize) -> Option<u32> {
        self.storage
            .as_ref()
            .get_ref()
            .descriptors
            .get(index)
            .map(TxDescriptor::buffer_address)
    }

    pub fn cancel(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.clear_to_free(AmpduDmaState::Reserved)
    }

    pub fn mark_completed(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.transition(AmpduDmaState::HardwareOwned, AmpduDmaState::Completed)
    }

    pub fn mark_detached(
        &mut self,
        detached: MacTxQueueDetached<'_>,
    ) -> Result<(), AmpduDmaStorageError> {
        if !detached.confirms_descriptor_head(self.binding.descriptors.base()) {
            return Err(AmpduDmaStorageError::Address);
        }
        self.transition(AmpduDmaState::Completed, AmpduDmaState::Detached)
    }

    /// Detach a hardware-owned chain the queue left without a completion (a
    /// collision or a settled timeout abort), keeping its buffers.
    pub fn mark_aborted_detached(
        &mut self,
        detached: MacTxQueueDetached<'_>,
    ) -> Result<(), AmpduDmaStorageError> {
        if !detached.confirms_descriptor_head(self.binding.descriptors.base()) {
            return Err(AmpduDmaStorageError::Address);
        }
        self.transition(AmpduDmaState::HardwareOwned, AmpduDmaState::Detached)
    }

    pub fn detached_buffer(
        &self,
        index: usize,
    ) -> Result<&[u8; BUFFER_SIZE], AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Detached {
            return Err(AmpduDmaStorageError::State);
        }
        self.storage
            .as_ref()
            .get_ref()
            .buffers
            .get(index)
            .map(|buffer| &buffer.0)
            .ok_or(AmpduDmaStorageError::Count)
    }

    pub fn release_detached(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.clear_to_free(AmpduDmaState::Detached)
    }

    pub fn release_aborted(
        &mut self,
        detached: MacTxQueueDetached<'_>,
    ) -> Result<(), AmpduDmaStorageError> {
        if !detached.confirms_descriptor_head(self.binding.descriptors.base()) {
            return Err(AmpduDmaStorageError::Address);
        }
        self.clear_to_free(AmpduDmaState::HardwareOwned)
    }

    pub fn quarantine(&mut self) {
        *self.storage.as_mut().project().state = AmpduDmaState::ResetRequired;
    }

    fn transition(
        &mut self,
        expected: AmpduDmaState,
        next: AmpduDmaState,
    ) -> Result<(), AmpduDmaStorageError> {
        let storage = self.storage.as_mut().project();
        if *storage.state != expected {
            return Err(AmpduDmaStorageError::State);
        }
        *storage.state = next;
        Ok(())
    }

    fn clear_to_free(&mut self, expected: AmpduDmaState) -> Result<(), AmpduDmaStorageError> {
        if self.state() != expected {
            return Err(AmpduDmaStorageError::State);
        }
        let published_count = self.storage.as_ref().get_ref().published_count;
        for descriptor in self
            .storage
            .as_ref()
            .get_ref()
            .descriptors
            .iter()
            .take(published_count)
        {
            descriptor.clear();
        }
        let storage = self.storage.as_mut().project();
        *storage.published_count = 0;
        *storage.state = AmpduDmaState::Free;
        Ok(())
    }
}

/// Aggregate DMA owner which retains every externally referenced allocation.
///
/// The descriptor arena remains in static chip storage while `B` may be a
/// movable lease object whose underlying allocation is stable. Moving this
/// owner is safe: descriptor addresses never point at the lease object, only
/// at the allocation guaranteed by its unsafe [`StableDmaBacking`](oer_portable_memory::StableDmaBacking) contract.
///
/// The arena never automatically destroys a retained lease. Only the active
/// owner can release one after cancellation or proven detach. If that owner
/// is forgotten, destroying this arena leaks its leases and their allocations;
/// it cannot free memory still visible to DMA. No destructor is required for
/// this safety property, including the arena's own destructor.
/// The [`StableDmaBacking`](oer_portable_memory::StableDmaBacking) contract requires the allocation and its exclusive
/// reservation to survive a leaked lease; an ordinary borrowed backing is not
/// sufficient. CPU-only metadata may still have a shorter lifetime because no
/// forgotten owner accesses or destroys it.
pub struct RetainedAmpduDmaStorage<B, const SLOTS: usize> {
    backings: [Option<ManuallyDrop<B>>; SLOTS],
    backing_identities: [Option<RetainedDmaBackingIdentity>; SLOTS],
    backing_descriptors: [Option<RetainedDmaDescriptor>; SLOTS],
    active_backing_indices: [u8; SLOTS],
}

impl<B, const SLOTS: usize> RetainedAmpduDmaStorage<B, SLOTS> {
    pub const fn new() -> Self {
        Self {
            backings: [const { None }; SLOTS],
            backing_identities: [None; SLOTS],
            backing_descriptors: [None; SLOTS],
            active_backing_indices: [0; SLOTS],
        }
    }
}

impl<B, const SLOTS: usize> Default for RetainedAmpduDmaStorage<B, SLOTS> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct RetainedAmpduDma<'retention, B, const SLOTS: usize, const BUFFER_SIZE: usize> {
    dma: Option<PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>>,
    retention: Option<&'retention mut RetainedAmpduDmaStorage<B, SLOTS>>,
    active_count: usize,
    held: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RetainedDmaBackingIdentity {
    generation: u64,
    address: usize,
    len: usize,
    /// The region's DMA addresses, `None` when it has none in the arena's
    /// space.
    dma_region: PlacedRegion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RetainedDmaDescriptor {
    buffer_address: DmaWindowAddress,
    word0: WifiTxDmaDescriptorControl,
}

impl<'retention, B, const SLOTS: usize, const BUFFER_SIZE: usize>
    RetainedAmpduDma<'retention, B, SLOTS, BUFFER_SIZE>
{
    pub fn new(
        dma: PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>,
        retention: &'retention mut RetainedAmpduDmaStorage<B, SLOTS>,
    ) -> Self {
        Self {
            dma: Some(dma),
            retention: Some(retention),
            active_count: 0,
            held: 0,
        }
    }

    /// The arena's state; an owner without its arena (never a live one,
    /// see [`AmpduDmaStorageError::Retired`]) publishes nothing and reads
    /// as [`AmpduDmaState::ResetRequired`].
    pub fn state(&self) -> AmpduDmaState {
        self.dma
            .as_ref()
            .map_or(AmpduDmaState::ResetRequired, PinnedAmpduDmaStorage::state)
    }

    pub const fn held_backing_count(&self) -> usize {
        self.held
    }

    /// The whole region of the lease at logical `position` in any state,
    /// for host observation of what a model publication carries.
    #[cfg(any(test, feature = "test-support"))]
    pub fn observed_region_mut(&mut self, position: usize) -> Option<&mut [u8]>
    where
        B: PlacedBacking,
    {
        let retention = self.retention.as_deref_mut()?;
        let index = usize::from(*retention.active_backing_indices.get(position)?);
        Some(
            retention
                .backings
                .get_mut(index)?
                .as_mut()?
                .stable_dma_region()
                .into_mut_slice(),
        )
    }

    /// The descriptor word bound to the lease at logical `position`, for
    /// host observation.
    #[cfg(any(test, feature = "test-support"))]
    pub fn bound_word0(&self, position: usize) -> Option<WifiTxDmaDescriptorControl> {
        let retention = self.retention().ok()?;
        let index = usize::from(*retention.active_backing_indices.get(position)?);
        retention
            .backing_descriptors
            .get(index)
            .copied()
            .flatten()
            .map(|descriptor| descriptor.word0)
    }

    pub fn descriptor_head(&self) -> Result<u32, AmpduDmaStorageError> {
        Ok(self.dma()?.binding().descriptor_head())
    }

    pub fn begin(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut()?.begin()?;
        for (index, slot) in self
            .retention_mut()?
            .active_backing_indices
            .iter_mut()
            .enumerate()
        {
            *slot = index as u8;
        }
        self.active_count = 0;
        Ok(())
    }

    /// Cancel the reserved chain and hand every retained lease to
    /// `returned`, in the order they were pushed.
    pub fn cancel(&mut self, returned: impl FnMut(B)) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut()?.cancel()?;
        self.return_backings(returned);
        Ok(())
    }

    pub fn mark_completed(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut()?.mark_completed()
    }

    pub fn mark_detached(
        &mut self,
        detached: MacTxQueueDetached<'_>,
    ) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut()?.mark_detached(detached)
    }

    /// Detach a chain the queue left without a completion, keeping every
    /// lease for a retained retry or a release.
    pub fn mark_aborted_detached(
        &mut self,
        detached: MacTxQueueDetached<'_>,
    ) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut()?.mark_aborted_detached(detached)
    }

    /// Keep the leases at the logical positions of `keep` for another
    /// publication, compacted in their order to positions `0..k`, and hand
    /// every other lease to `returned` in position order: the detached chain
    /// is reserved again with its kept descriptor images unchanged, and
    /// further leases append after them. Returns `k`.
    ///
    /// Only a chain whose leases sit at their own positions (every chain
    /// [`Self::push_written`] built) is retained this way.
    pub fn retain_detached(
        &mut self,
        keep: u64,
        mut returned: impl FnMut(B),
    ) -> Result<usize, AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Detached {
            return Err(AmpduDmaStorageError::State);
        }
        let count = self.active_count;
        let outside = u32::try_from(count)
            .ok()
            .and_then(|count| keep.checked_shr(count))
            .unwrap_or(0);
        if self.held != count || outside != 0 {
            return Err(AmpduDmaStorageError::Count);
        }
        let in_place = self
            .retention()?
            .active_backing_indices
            .iter()
            .take(count)
            .enumerate()
            .all(|(position, index)| usize::from(*index) == position);
        if !in_place {
            return Err(AmpduDmaStorageError::State);
        }
        self.dma_mut()?
            .transition(AmpduDmaState::Detached, AmpduDmaState::Reserved)?;
        let retention = self.retention_mut()?;
        let mut kept = 0_usize;
        for position in 0..count {
            let selected = u32::try_from(position)
                .ok()
                .and_then(|position| keep.checked_shr(position))
                .is_some_and(|bits| bits & 1 != 0);
            let backing = retention.backings.get_mut(position).and_then(Option::take);
            let identity = retention
                .backing_identities
                .get_mut(position)
                .and_then(Option::take);
            let descriptor = retention
                .backing_descriptors
                .get_mut(position)
                .and_then(Option::take);
            if !selected {
                if let Some(backing) = backing {
                    returned(ManuallyDrop::into_inner(backing));
                }
                continue;
            }
            // The target lies at or before `position`: it was emptied above.
            if let Some(slot) = retention.backings.get_mut(kept) {
                *slot = backing;
            }
            if let Some(slot) = retention.backing_identities.get_mut(kept) {
                *slot = identity;
            }
            if let Some(slot) = retention.backing_descriptors.get_mut(kept) {
                *slot = descriptor;
            }
            kept = kept.saturating_add(1);
        }
        for (position, index) in retention.active_backing_indices.iter_mut().enumerate() {
            *index = u8::try_from(position).unwrap_or(u8::MAX);
        }
        self.held = kept;
        self.active_count = kept;
        Ok(kept)
    }

    /// Release the detached chain and hand every retained lease to
    /// `returned`, in the order they were pushed.
    pub fn release_detached(
        &mut self,
        returned: impl FnMut(B),
    ) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut()?.release_detached()?;
        self.return_backings(returned);
        Ok(())
    }

    /// Release the aborted chain the detach proof names and hand every
    /// retained lease to `returned`, in the order they were pushed.
    pub fn release_aborted(
        &mut self,
        detached: MacTxQueueDetached<'_>,
        returned: impl FnMut(B),
    ) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut()?.release_aborted(detached)?;
        self.return_backings(returned);
        Ok(())
    }

    pub fn quarantine(&mut self) {
        if let Some(dma) = self.dma.as_mut() {
            dma.quarantine();
        }
    }

    #[allow(clippy::result_large_err)]
    pub fn try_into_parts(
        mut self,
    ) -> Result<
        (
            PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>,
            &'retention mut RetainedAmpduDmaStorage<B, SLOTS>,
        ),
        Self,
    > {
        if self.state() != AmpduDmaState::Free || self.held != 0 {
            return Err(self);
        }
        match (self.dma.take(), self.retention.take()) {
            (Some(dma), Some(retention)) => Ok((dma, retention)),
            (dma, retention) => {
                self.dma = dma;
                self.retention = retention;
                Err(self)
            }
        }
    }

    fn dma(&self) -> Result<&PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>, AmpduDmaStorageError> {
        self.dma.as_ref().ok_or(AmpduDmaStorageError::Retired)
    }

    fn dma_mut(
        &mut self,
    ) -> Result<&mut PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>, AmpduDmaStorageError> {
        self.dma.as_mut().ok_or(AmpduDmaStorageError::Retired)
    }

    fn retention(&self) -> Result<&RetainedAmpduDmaStorage<B, SLOTS>, AmpduDmaStorageError> {
        self.retention
            .as_deref()
            .ok_or(AmpduDmaStorageError::Retired)
    }

    fn retention_mut(
        &mut self,
    ) -> Result<&mut RetainedAmpduDmaStorage<B, SLOTS>, AmpduDmaStorageError> {
        self.retention
            .as_deref_mut()
            .ok_or(AmpduDmaStorageError::Retired)
    }

    /// Hand every retained lease to `returned` in push order; only after
    /// the chain left the hardware.
    fn return_backings(&mut self, mut returned: impl FnMut(B)) {
        let held = self.held;
        if let Some(retention) = self.retention.as_deref_mut() {
            for ((backing, identity), descriptor) in retention
                .backings
                .iter_mut()
                .zip(retention.backing_identities.iter_mut())
                .zip(retention.backing_descriptors.iter_mut())
                .take(held)
            {
                if let Some(backing) = backing.take().map(ManuallyDrop::into_inner) {
                    returned(backing);
                }
                *identity = None;
                *descriptor = None;
            }
        }
        self.held = 0;
        self.active_count = 0;
    }

    fn drop_backings(&mut self) {
        self.return_backings(drop);
    }

    #[allow(
        clippy::mem_forget,
        reason = "a backing the hardware may still read is leaked, never returned or freed"
    )]
    fn forget_backings(&mut self) {
        let held = self.held;
        if let Some(retention) = self.retention.as_deref_mut() {
            for ((backing, identity), descriptor) in retention
                .backings
                .iter_mut()
                .zip(retention.backing_identities.iter_mut())
                .zip(retention.backing_descriptors.iter_mut())
                .take(held)
            {
                if let Some(backing) = backing.take() {
                    mem::forget(ManuallyDrop::into_inner(backing));
                }
                *identity = None;
                *descriptor = None;
            }
        }
        self.held = 0;
        self.active_count = 0;
    }

    fn next_lease_generation(&mut self) -> Result<u64, AmpduDmaStorageError> {
        let dma = self.dma_mut()?;
        let generation = dma
            .storage
            .as_ref()
            .get_ref()
            .lease_generation
            .checked_add(1)
            .ok_or(AmpduDmaStorageError::GenerationExhausted)?;
        *dma.storage.as_mut().project().lease_generation = generation;
        Ok(generation)
    }
}

impl<'retention, B: PlacedBacking, const SLOTS: usize, const BUFFER_SIZE: usize>
    RetainedAmpduDma<'retention, B, SLOTS, BUFFER_SIZE>
{
    fn prepare_backings_for_dma_read(&mut self) -> Result<(), AmpduDmaStorageError> {
        let held = self.held;
        for backing in self.retention_mut()?.backings.iter_mut().take(held) {
            backing
                .as_mut()
                .ok_or(AmpduDmaStorageError::StaleBacking)?
                .prepare_for_dma_read();
        }
        Ok(())
    }

    /// Retain `backing` at the next position once `write` accepts it, and
    /// bind its descriptor image at once: `write` receives the backing's
    /// whole region and names the descriptor's buffer in it
    /// ([`WrittenBuffer`]) with a record `T` that `commit` takes once every
    /// check of the owner passed, the last fallible step before the lease
    /// is retained; both borrow the caller's `context` in turn. Every later
    /// publication relinks that image without sampling the region again. A
    /// refusal, by `write`, `commit` or the owner, returns the backing as
    /// `write` left it and retains nothing.
    pub fn push_written<C, T, R>(
        &mut self,
        mut backing: B,
        context: &mut C,
        write: impl FnOnce(&mut C, &mut [u8]) -> Result<(WrittenBuffer, T), R>,
        commit: impl FnOnce(&mut C, T) -> Result<(), R>,
    ) -> Result<(), (B, PushRefused<R>)> {
        let index = match self.vacant_position() {
            Ok(index) => index,
            Err(error) => return Err((backing, PushRefused::Storage(error))),
        };
        let Ok(dma_region) = backing.dma_region() else {
            return Err((backing, PushRefused::Storage(AmpduDmaStorageError::Address)));
        };
        let bytes = backing.stable_dma_region().into_mut_slice();
        let (address, len) = (bytes.as_ptr().addr(), bytes.len());
        let (written, record) = match write(context, bytes) {
            Ok(written) => written,
            Err(refused) => return Err((backing, PushRefused::Written(refused))),
        };
        let identity = RetainedDmaBackingIdentity {
            generation: 0,
            address,
            len,
            dma_region,
        };
        let descriptor = match bound_descriptor(identity, written) {
            Ok(descriptor) => descriptor,
            Err(error) => return Err((backing, PushRefused::Storage(error))),
        };
        let generation = match self.next_lease_generation() {
            Ok(generation) => generation,
            Err(error) => return Err((backing, PushRefused::Storage(error))),
        };
        let Some(retention) = self.retention.as_deref_mut() else {
            return Err((backing, PushRefused::Storage(AmpduDmaStorageError::Retired)));
        };
        let Some(slot) = retention.backings.get_mut(index) else {
            return Err((backing, PushRefused::Storage(AmpduDmaStorageError::Count)));
        };
        if let Err(refused) = commit(context, record) {
            return Err((backing, PushRefused::Written(refused)));
        }
        *slot = Some(ManuallyDrop::new(backing));
        // `vacant_position` found all three places of `index` empty.
        if let Some(slot) = retention.backing_identities.get_mut(index) {
            *slot = Some(RetainedDmaBackingIdentity {
                generation,
                ..identity
            });
        }
        if let Some(slot) = retention.backing_descriptors.get_mut(index) {
            *slot = Some(descriptor);
        }
        if let Some(slot) = retention.active_backing_indices.get_mut(index) {
            *slot = u8::try_from(index).unwrap_or(u8::MAX);
        }
        self.held = index.saturating_add(1);
        self.active_count = self.held;
        Ok(())
    }

    /// The next position of a reserved chain whose leases sit at their own
    /// positions, when its three places are empty.
    fn vacant_position(&self) -> Result<usize, AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Reserved {
            return Err(AmpduDmaStorageError::State);
        }
        let index = self.held;
        if index >= SLOTS || index != self.active_count {
            return Err(AmpduDmaStorageError::Count);
        }
        self.dma()?
            .binding
            .descriptor(index)
            .ok_or(AmpduDmaStorageError::Address)?;
        let retention = self.retention()?;
        let vacant = retention.backings.get(index).is_some_and(Option::is_none)
            && retention
                .backing_identities
                .get(index)
                .is_some_and(Option::is_none)
            && retention
                .backing_descriptors
                .get(index)
                .is_some_and(Option::is_none);
        if !vacant {
            return Err(AmpduDmaStorageError::StaleBacking);
        }
        Ok(index)
    }

    /// The whole region of the lease at logical `position` while software
    /// owns the chain: reserved or detached.
    pub fn software_region_mut(
        &mut self,
        position: usize,
    ) -> Result<&mut [u8], AmpduDmaStorageError> {
        if !matches!(
            self.state(),
            AmpduDmaState::Reserved | AmpduDmaState::Detached
        ) {
            return Err(AmpduDmaStorageError::State);
        }
        if position >= self.active_count {
            return Err(AmpduDmaStorageError::Count);
        }
        let retention = self.retention_mut()?;
        let index = usize::from(
            *retention
                .active_backing_indices
                .get(position)
                .ok_or(AmpduDmaStorageError::Count)?,
        );
        Ok(retention
            .backings
            .get_mut(index)
            .and_then(Option::as_mut)
            .ok_or(AmpduDmaStorageError::StaleBacking)?
            .stable_dma_region()
            .into_mut_slice())
    }

    /// Borrow a detached range only after the queue-detach proof was consumed.
    pub fn detached_region_mut(
        &mut self,
        address: usize,
        capacity: usize,
    ) -> Result<&mut [u8], AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Detached {
            return Err(AmpduDmaStorageError::State);
        }
        let held = self.held;
        for backing in self.retention_mut()?.backings.iter_mut().take(held) {
            let Some(backing) = backing.as_mut() else {
                continue;
            };
            let bytes = backing.stable_dma_region().into_mut_slice();
            let Some(offset) = address.checked_sub(bytes.as_ptr().addr()) else {
                continue;
            };
            let Some(end) = offset.checked_add(capacity) else {
                continue;
            };
            if let Some(range) = bytes.get_mut(offset..end) {
                return Ok(range);
            }
        }
        Err(AmpduDmaStorageError::Address)
    }

    /// Publish the descriptor images bound to the first `count` retained
    /// leases in logical order; any lease after them stays retained,
    /// unpublished, until the chain's release or retention.
    pub fn publish_retained_chain(
        &mut self,
        count: usize,
    ) -> Result<
        RetainedAmpduDmaPublication<'_, 'retention, B, SLOTS, BUFFER_SIZE>,
        AmpduDmaStorageError,
    > {
        if self.state() != AmpduDmaState::Reserved {
            return Err(AmpduDmaStorageError::State);
        }
        if count == 0 || count > SLOTS || count > self.active_count {
            return Err(AmpduDmaStorageError::Count);
        }

        let mut descriptors = [None; SLOTS];
        let retention = self.retention()?;
        for ((logical_index, destination), backing_index) in descriptors
            .iter_mut()
            .enumerate()
            .zip(retention.active_backing_indices.iter().copied())
            .take(count)
        {
            let descriptor = retention
                .backing_descriptors
                .get(usize::from(backing_index))
                .copied()
                .flatten()
                .ok_or(AmpduDmaStorageError::StaleBacking)?;
            self.dma()?
                .binding
                .descriptor_address(logical_index)
                .ok_or(AmpduDmaStorageError::Address)?;
            *destination = Some(descriptor);
        }

        // Retry compaction can also mutate retained metadata, so every
        // publication (not only the first one) must re-establish visibility.
        self.prepare_backings_for_dma_read()?;

        let dma = self.dma_mut()?;
        let arena = dma.storage.as_ref().get_ref();
        for (logical_index, (descriptor, slot)) in descriptors
            .iter()
            .copied()
            .zip(arena.descriptors.iter())
            .take(count)
            .enumerate()
        {
            let descriptor = descriptor.ok_or(AmpduDmaStorageError::StaleBacking)?;
            let following = logical_index.saturating_add(1);
            let next = if following == count {
                None
            } else {
                Some(
                    dma.binding
                        .descriptor(following)
                        .ok_or(AmpduDmaStorageError::Address)?,
                )
            };
            let word0 = descriptor.word0.with_tx_last(next.is_none());
            slot.publish_owned(word0, descriptor.buffer_address, next);
        }
        *dma.storage.as_mut().project().published_count = count;

        Ok(RetainedAmpduDmaPublication {
            dma,
            _owner: PhantomData,
        })
    }
}

impl<B, const SLOTS: usize, const BUFFER_SIZE: usize> Drop
    for RetainedAmpduDma<'_, B, SLOTS, BUFFER_SIZE>
{
    fn drop(&mut self) {
        if self.retention.is_none() {
            debug_assert!(self.dma.is_none() && self.held == 0);
            return;
        }
        if self.dma.is_none() {
            self.drop_backings();
            return;
        }
        if matches!(
            self.state(),
            AmpduDmaState::HardwareOwned | AmpduDmaState::Completed | AmpduDmaState::ResetRequired
        ) {
            // Never release a lease which the peripheral may still reference.
            // Forget it first so the lower pinned DMA owner's fail-closed
            // destructor cannot run a backing destructor while rejecting the
            // still-active lifecycle state.
            self.forget_backings();
        } else {
            self.drop_backings();
        }
    }
}

/// The descriptor buffer a [`RetainedAmpduDma::push_written`] writer names
/// in the region it received: `capacity` bytes from `offset`, of which the
/// DMA reads `transfer_length`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WrittenBuffer {
    pub offset: usize,
    pub capacity: u32,
    pub transfer_length: u32,
}

/// Why [`RetainedAmpduDma::push_written`] returned its backing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PushRefused<R> {
    /// The owner refused the backing or the buffer its writer named.
    Storage(AmpduDmaStorageError),
    /// The writer refused.
    Written(R),
}

/// The descriptor image of `written` inside the region `identity` names.
fn bound_descriptor(
    identity: RetainedDmaBackingIdentity,
    written: WrittenBuffer,
) -> Result<RetainedDmaDescriptor, AmpduDmaStorageError> {
    let WrittenBuffer {
        offset,
        capacity,
        transfer_length,
    } = written;
    if capacity == 0 || transfer_length == 0 || transfer_length > capacity {
        return Err(AmpduDmaStorageError::InvalidLength);
    }
    let end = usize::try_from(capacity)
        .ok()
        .and_then(|capacity| offset.checked_add(capacity))
        .ok_or(AmpduDmaStorageError::Address)?;
    if end > identity.len {
        return Err(AmpduDmaStorageError::Address);
    }
    Ok(RetainedDmaDescriptor {
        buffer_address: external_dma_address(identity, offset)?,
        word0: tx_owned_word(capacity, transfer_length)
            .ok_or(AmpduDmaStorageError::InvalidLength)?,
    })
}

/// The DMA address `offset` bytes into a retained region. The region is a
/// [`PlacedBacking`]'s, so it lies in the DMA window; that `capacity` bytes
/// from `offset` stay inside it is checked by the caller against the
/// region's length.
fn external_dma_address(
    identity: RetainedDmaBackingIdentity,
    offset: usize,
) -> Result<DmaWindowAddress, AmpduDmaStorageError> {
    let offset = u32::try_from(offset).map_err(|_| AmpduDmaStorageError::Address)?;
    identity
        .dma_region
        .at(offset)
        .ok_or(AmpduDmaStorageError::Address)
}

pub struct RetainedAmpduDmaPublication<
    'owner,
    'retention,
    B,
    const SLOTS: usize,
    const BUFFER_SIZE: usize,
> {
    /// The arena of the owner this token mutably borrows for its life.
    dma: &'owner mut PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>,
    _owner: PhantomData<&'owner mut RetainedAmpduDma<'retention, B, SLOTS, BUFFER_SIZE>>,
}

#[allow(
    unsafe_code,
    reason = "publication token is coupled to the owner retaining every external lease"
)]
// SAFETY: this token exists only after the retaining owner published the
// complete aggregate chain; it mutably borrows that owner's arena, so the
// owner, which retains every external lease, can neither release nor change
// one while it lives, and hardware cannot own the chain before `commit`.
unsafe impl<B: PlacedBacking, const SLOTS: usize, const BUFFER_SIZE: usize>
    PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>
    for RetainedAmpduDmaPublication<'_, '_, B, SLOTS, BUFFER_SIZE>
{
    fn descriptor_head(&self) -> DmaRef<WifiTxDmaDescriptor> {
        self.dma.binding.descriptors.first()
    }
}

impl<B, const SLOTS: usize, const BUFFER_SIZE: usize>
    RetainedAmpduDmaPublication<'_, '_, B, SLOTS, BUFFER_SIZE>
{
    pub fn commit<F>(self, start: F)
    where
        F: FnOnce(&AmpduDmaStart<'_>),
    {
        let dma = self.dma;
        *dma.storage.as_mut().project().state = AmpduDmaState::HardwareOwned;
        start(&AmpduDmaStart {
            binding: &dma.binding,
        });
    }
}

impl<B: PlacedBacking, const SLOTS: usize, const BUFFER_SIZE: usize> CommitOwner
    for RetainedAmpduDmaPublication<'_, '_, B, SLOTS, BUFFER_SIZE>
{
}

// The token holds the one arena `AmpduDmaStart` borrows from, so the authority
// names the chain `descriptor_head` reads.
impl<B: PlacedBacking, const SLOTS: usize, const BUFFER_SIZE: usize> CommitTxDma
    for RetainedAmpduDmaPublication<'_, '_, B, SLOTS, BUFFER_SIZE>
{
    type Started<'a> = AmpduDmaStart<'a>;

    fn commit(self, start: impl for<'a> FnOnce(&Self::Started<'a>)) {
        RetainedAmpduDmaPublication::commit(self, start);
    }
}

pub struct AmpduDmaPublication<'owner, const SLOTS: usize, const BUFFER_SIZE: usize> {
    owner: &'owner mut PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE>,
}

#[allow(
    unsafe_code,
    reason = "publication token retains a validated internal aggregate chain"
)]
// SAFETY: this token exists only after the pinned owner published and
// validated the internal aggregate chain; it mutably borrows that owner, and
// hardware cannot own the chain before `commit`.
unsafe impl<const SLOTS: usize, const BUFFER_SIZE: usize> PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>
    for AmpduDmaPublication<'_, SLOTS, BUFFER_SIZE>
{
    fn descriptor_head(&self) -> DmaRef<WifiTxDmaDescriptor> {
        self.owner.binding.descriptors.first()
    }
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> AmpduDmaPublication<'_, SLOTS, BUFFER_SIZE> {
    pub fn commit<F>(self, start: F)
    where
        F: FnOnce(&AmpduDmaStart<'_>),
    {
        let owner = self.owner;
        *owner.storage.as_mut().project().state = AmpduDmaState::HardwareOwned;
        start(&AmpduDmaStart {
            binding: &owner.binding,
        });
    }
}

#[cfg(test)]
mod tests;
