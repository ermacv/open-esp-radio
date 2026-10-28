//! Pinned descriptor and backing ownership for one TX A-MPDU.
//!
//! Internally buffered aggregates retain their static arena directly.
//! External zero-copy aggregates receive prepare/start authority only through
//! an owner which also retains every referenced [`StableDmaBacking`](oer_memory::StableDmaBacking) lease.

use crate::memory::dma_address::{PlacedRegion, PlacedSlots, PlacedTable};
use crate::ieee80211::dma::space::DmaSpace;
use core::{marker::PhantomPinned, mem, mem::ManuallyDrop, pin::Pin};

use crate::memory::dma_static::{Placed, PlacedBacking};
use crate::types::MacTxQueueDetached;
use oer_memory::{HardwareOwnedTxDma, PreparedTxDma, StableDmaRange};
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
}

/// One internal A-MPDU descriptor entry, excluding its derived addresses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduInternalDescriptor {
    pub buffer_capacity: u32,
    pub transfer_length: u32,
}

/// Non-forgeable identity of one lease retained by an aggregate owner.
///
/// The generation is allocated by the pinned descriptor arena and therefore
/// survives conversion between idle and retained owners. Moving or replacing
/// a backing never revives an older identity.
#[derive(Debug, Eq, PartialEq)]
pub struct RetainedDmaBacking {
    descriptor_head: u32,
    index: u8,
    generation: u64,
}

impl RetainedDmaBacking {
    pub const fn index(&self) -> u8 {
        self.index
    }

    /// Bind one descriptor range to this retained lease identity.
    ///
    /// Current-owner validation is deferred until publication, immediately
    /// before descriptor mutation. Private identity fields still prove that
    /// this complete range belonged to the accepted stable allocation.
    pub fn external_descriptor(
        &self,
        address: usize,
        buffer_capacity: u32,
        transfer_length: u32,
    ) -> Result<AmpduExternalDescriptor<'_>, AmpduDmaStorageError> {
        if buffer_capacity == 0 || transfer_length == 0 || transfer_length > buffer_capacity {
            return Err(AmpduDmaStorageError::InvalidLength);
        }
        Ok(AmpduExternalDescriptor {
            backing: Some(self),
            address,
            buffer_capacity,
            transfer_length,
        })
    }
}

/// One descriptor backed by a retained external DMA lease.
///
/// Fields are private so safe code can obtain an entry only after the DMA
/// owner has validated a current [`RetainedDmaBacking`] identity and range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduExternalDescriptor<'backing> {
    backing: Option<&'backing RetainedDmaBacking>,
    address: usize,
    buffer_capacity: u32,
    transfer_length: u32,
}

impl AmpduExternalDescriptor<'_> {
    /// Invalid fixed-array filler. Publication always rejects this value.
    pub const EMPTY: Self = Self {
        backing: None,
        address: 0,
        buffer_capacity: 0,
        transfer_length: 0,
    };
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
/// at the allocation guaranteed by its unsafe [`StableDmaBacking`](oer_memory::StableDmaBacking) contract.
///
/// The arena never automatically destroys a retained lease. Only the active
/// owner can release one after cancellation or proven detach. If that owner
/// is forgotten, destroying this arena leaks its leases and their allocations;
/// it cannot free memory still visible to DMA. No destructor is required for
/// this safety property, including the arena's own destructor.
/// The [`StableDmaBacking`](oer_memory::StableDmaBacking) contract requires the allocation and its exclusive
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

    pub fn state(&self) -> AmpduDmaState {
        self.dma().state()
    }

    pub const fn held_backing_count(&self) -> usize {
        self.held
    }

    pub fn descriptor_head(&self) -> u32 {
        self.dma().binding().descriptor_head()
    }

    pub fn begin(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut().begin()?;
        for (index, slot) in self
            .retention_mut()
            .active_backing_indices
            .iter_mut()
            .enumerate()
        {
            *slot = index as u8;
        }
        self.active_count = 0;
        Ok(())
    }

    /// Re-reserve a detached aggregate for a selective BlockAck retry.
    ///
    /// All leases stay retained. The new descriptor chain may reference a
    /// compacted subset of them, while acknowledged frames remain owned until
    /// the retry reaches its final detach/release edge.
    #[cfg(any(test, feature = "test-support"))]
    pub fn begin_retry(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut()
            .transition(AmpduDmaState::Detached, AmpduDmaState::Reserved)
    }

    /// Undo the most recent reserved backing insertion.
    ///
    /// Aggregate metadata is prepared by the upper MAC after the lease has
    /// entered this owner.  If that preparation rejects the frame, this
    /// strictly-LIFO edge returns the lease without leaving a hole in the
    /// backing-index space later published to DMA descriptors.
    pub fn pop_last_backing(
        &mut self,
        backing: RetainedDmaBacking,
    ) -> Result<B, AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Reserved {
            return Err(AmpduDmaStorageError::State);
        }
        let index = usize::from(backing.index);
        if self.held.checked_sub(1) != Some(index) {
            return Err(AmpduDmaStorageError::Count);
        }
        self.validate_backing(&backing)?;
        let value = self.retention_mut().backings[index]
            .take()
            .map(ManuallyDrop::into_inner)
            .ok_or(AmpduDmaStorageError::Count)?;
        self.retention_mut().backing_identities[index] = None;
        self.retention_mut().backing_descriptors[index] = None;
        self.held = index;
        Ok(value)
    }

    pub fn reserved_backing_mut(
        &mut self,
        backing: &RetainedDmaBacking,
    ) -> Result<&mut B, AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Reserved {
            return Err(AmpduDmaStorageError::State);
        }
        self.validate_backing(backing)?;
        self.retention_mut()
            .backings
            .get_mut(usize::from(backing.index))
            .and_then(Option::as_mut)
            .map(|backing| &mut **backing)
            .ok_or(AmpduDmaStorageError::Count)
    }

    pub fn detached_backing_mut(
        &mut self,
        backing: &RetainedDmaBacking,
    ) -> Result<&mut B, AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Detached {
            return Err(AmpduDmaStorageError::State);
        }
        self.validate_backing(backing)?;
        self.retention_mut()
            .backings
            .get_mut(usize::from(backing.index))
            .and_then(Option::as_mut)
            .map(|backing| &mut **backing)
            .ok_or(AmpduDmaStorageError::Count)
    }

    pub fn cancel(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut().cancel()?;
        self.drop_backings();
        Ok(())
    }

    pub fn mark_completed(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut().mark_completed()
    }

    pub fn mark_detached(
        &mut self,
        detached: MacTxQueueDetached<'_>,
    ) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut().mark_detached(detached)
    }

    pub fn release_detached(&mut self) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut().release_detached()?;
        self.drop_backings();
        Ok(())
    }

    pub fn release_aborted(
        &mut self,
        detached: MacTxQueueDetached<'_>,
    ) -> Result<(), AmpduDmaStorageError> {
        self.dma_mut().release_aborted(detached)?;
        self.drop_backings();
        Ok(())
    }

    pub fn quarantine(&mut self) {
        self.dma_mut().quarantine();
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
        let dma = self
            .dma
            .take()
            .expect("retained aggregate owner contains its DMA arena");
        let retention = self
            .retention
            .take()
            .expect("retained aggregate owner contains its lease arena");
        Ok((dma, retention))
    }

    fn dma(&self) -> &PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE> {
        self.dma
            .as_ref()
            .expect("retained aggregate owner contains its DMA arena")
    }

    fn dma_mut(&mut self) -> &mut PinnedAmpduDmaStorage<SLOTS, BUFFER_SIZE> {
        self.dma
            .as_mut()
            .expect("retained aggregate owner contains its DMA arena")
    }

    fn retention(&self) -> &RetainedAmpduDmaStorage<B, SLOTS> {
        self.retention
            .as_deref()
            .expect("retained aggregate owner contains its lease arena")
    }

    fn retention_mut(&mut self) -> &mut RetainedAmpduDmaStorage<B, SLOTS> {
        self.retention
            .as_deref_mut()
            .expect("retained aggregate owner contains its lease arena")
    }

    fn drop_backings(&mut self) {
        for index in 0..self.held {
            drop(
                self.retention_mut().backings[index]
                    .take()
                    .map(ManuallyDrop::into_inner),
            );
            self.retention_mut().backing_identities[index] = None;
            self.retention_mut().backing_descriptors[index] = None;
        }
        self.held = 0;
        self.active_count = 0;
    }

    #[allow(
        clippy::mem_forget,
        reason = "a backing the hardware may still read is leaked, never returned or freed"
    )]
    fn forget_backings(&mut self) {
        for index in 0..self.held {
            if let Some(backing) = self.retention_mut().backings[index].take() {
                mem::forget(ManuallyDrop::into_inner(backing));
            }
            self.retention_mut().backing_identities[index] = None;
            self.retention_mut().backing_descriptors[index] = None;
        }
        self.held = 0;
        self.active_count = 0;
    }

    fn validate_backing(
        &self,
        backing: &RetainedDmaBacking,
    ) -> Result<RetainedDmaBackingIdentity, AmpduDmaStorageError> {
        if backing.descriptor_head != self.descriptor_head() {
            return Err(AmpduDmaStorageError::StaleBacking);
        }
        let identity = self
            .retention()
            .backing_identities
            .get(usize::from(backing.index))
            .copied()
            .flatten()
            .ok_or(AmpduDmaStorageError::StaleBacking)?;
        if identity.generation != backing.generation {
            return Err(AmpduDmaStorageError::StaleBacking);
        }
        Ok(identity)
    }

    fn next_lease_generation(&mut self) -> Result<u64, AmpduDmaStorageError> {
        let dma = self.dma_mut();
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
        for backing in &mut self.retention_mut().backings[..held] {
            backing
                .as_mut()
                .ok_or(AmpduDmaStorageError::StaleBacking)?
                .prepare_for_dma_read();
        }
        Ok(())
    }

    /// Retain one stable allocation and issue its non-forgeable identity.
    ///
    /// [`StableDmaBacking`](oer_memory::StableDmaBacking) guarantees that the accepted allocation remains at
    /// one address until the owner releases it, and remains allocated and
    /// exclusively reserved if the owner is quarantined or forgotten.
    pub fn push_backing(&mut self, backing: B) -> Result<RetainedDmaBacking, AmpduDmaStorageError> {
        let (backing, region) = self.push_backing_region(backing)?;
        let empty = region.is_empty();
        if empty {
            drop(self.pop_last_backing(backing)?);
            return Err(AmpduDmaStorageError::Address);
        }
        Ok(backing)
    }

    /// Retain a stable allocation and return its first mutable region view.
    ///
    /// This fuses identity sampling with the upper-MAC encoding borrow, so the
    /// normal commit path calls `stable_dma_region()` only once per new MPDU.
    pub fn push_backing_region(
        &mut self,
        backing: B,
    ) -> Result<(RetainedDmaBacking, &mut [u8]), AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Reserved {
            return Err(AmpduDmaStorageError::State);
        }
        let index = self.held;
        if index >= SLOTS {
            return Err(AmpduDmaStorageError::Count);
        }
        let backing_index = u8::try_from(index).map_err(|_| AmpduDmaStorageError::Count)?;
        // `held` is private and advances only when a moved backing enters
        // this owner; pop/release clear its identity and descriptor together.
        // The next bounded slot therefore cannot already have an owner.
        assert!(
            self.retention().backings[index].is_none()
                && self.retention().backing_identities[index].is_none()
                && self.retention().backing_descriptors[index].is_none(),
            "retained append owns its next vacant backing slot"
        );
        let descriptor_head = self.descriptor_head();
        let generation = self.next_lease_generation()?;
        let region = backing.dma_region();
        self.retention_mut().backings[index] = Some(ManuallyDrop::new(backing));
        // Count the lease before borrowing its region. If a backing's unsafe
        // implementation panics while producing the region, Drop still owns
        // and releases the inserted value. Split the retention fields so the
        // exact slice returned by the single stability call can also be
        // returned to the encoder without sampling the backing twice.
        self.held += 1;
        let dma_region = match region {
            Ok(region) => region,
            Err(_) => {
                self.held = index;
                let retention = self.retention_mut();
                let rejected = retention.backings[index]
                    .take()
                    .map(ManuallyDrop::into_inner);
                retention.backing_identities[index] = None;
                retention.backing_descriptors[index] = None;
                drop(rejected);
                return Err(AmpduDmaStorageError::Address);
            }
        };
        let retention = self
            .retention
            .as_deref_mut()
            .expect("retained aggregate owner contains its lease arena");
        let (backings, identities) = (&mut retention.backings, &mut retention.backing_identities);
        let bytes = backings[index]
            .as_mut()
            .expect("retained backing was inserted")
            .stable_dma_region()
            .into_mut_slice();
        let identity = RetainedDmaBackingIdentity {
            generation,
            address: bytes.as_ptr().addr(),
            len: bytes.len(),
            dma_region,
        };
        identities[index] = Some(identity);
        Ok((
            RetainedDmaBacking {
                descriptor_head,
                index: backing_index,
                generation: identity.generation,
            },
            bytes,
        ))
    }

    /// Borrow a detached range through its retained identity in O(1).
    pub fn detached_backing_region_mut(
        &mut self,
        backing: &RetainedDmaBacking,
        address: usize,
        capacity: usize,
    ) -> Result<&mut [u8], AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Detached {
            return Err(AmpduDmaStorageError::State);
        }
        let identity = self.validate_backing(backing)?;
        let offset = address
            .checked_sub(identity.address)
            .ok_or(AmpduDmaStorageError::Address)?;
        let end = offset
            .checked_add(capacity)
            .filter(|end| *end <= identity.len)
            .ok_or(AmpduDmaStorageError::Address)?;
        let bytes = self.retention_mut().backings[usize::from(backing.index)]
            .as_mut()
            .ok_or(AmpduDmaStorageError::StaleBacking)?
            .stable_dma_region()
            .into_mut_slice();
        Ok(&mut bytes[offset..end])
    }

    /// Bind the validated descriptor image to a retained backing once.
    ///
    /// Publication can subsequently rebuild the hardware chain without
    /// reconstructing or revalidating address-bearing values in an upper
    /// layer. The descriptor remains private to this owner until every
    /// retained backing is released or quarantined.
    /// Consume the sole retained-backing identity when binding its descriptor.
    /// A rejected external image returns that exact identity for correction.
    pub fn commit_backing_descriptor(
        &mut self,
        backing: RetainedDmaBacking,
        address: usize,
        buffer_capacity: u32,
        transfer_length: u32,
    ) -> Result<(), (RetainedDmaBacking, AmpduDmaStorageError)> {
        self.commit_backing_descriptor_inner(&backing, address, buffer_capacity, transfer_length)
            .map_err(|error| (backing, error))
    }

    fn commit_backing_descriptor_inner(
        &mut self,
        backing: &RetainedDmaBacking,
        address: usize,
        buffer_capacity: u32,
        transfer_length: u32,
    ) -> Result<(), AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Reserved {
            return Err(AmpduDmaStorageError::State);
        }
        if usize::from(backing.index) != self.active_count {
            return Err(AmpduDmaStorageError::Count);
        }
        if buffer_capacity == 0 || transfer_length == 0 || transfer_length > buffer_capacity {
            return Err(AmpduDmaStorageError::InvalidLength);
        }
        let identity = self.validate_backing(backing)?;
        let capacity =
            usize::try_from(buffer_capacity).map_err(|_| AmpduDmaStorageError::Address)?;
        let offset = address
            .checked_sub(identity.address)
            .ok_or(AmpduDmaStorageError::Address)?;
        offset
            .checked_add(capacity)
            .filter(|end| *end <= identity.len)
            .ok_or(AmpduDmaStorageError::Address)?;
        let buffer_address = external_dma_address(identity, offset)?;
        let word0 = tx_owned_word(buffer_capacity, transfer_length)
            .ok_or(AmpduDmaStorageError::InvalidLength)?;
        let slot = self
            .retention_mut()
            .backing_descriptors
            .get_mut(usize::from(backing.index))
            .ok_or(AmpduDmaStorageError::Count)?;
        assert!(
            slot.is_none(),
            "consumed backing identity binds its descriptor once"
        );
        *slot = Some(RetainedDmaDescriptor {
            buffer_address,
            word0,
        });
        self.active_count += 1;
        Ok(())
    }

    /// Borrow one current logical MPDU after the queue-detach proof.
    pub fn detached_logical_region_mut(
        &mut self,
        logical_index: usize,
        address: usize,
        capacity: usize,
    ) -> Result<&mut [u8], AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Detached {
            return Err(AmpduDmaStorageError::State);
        }
        let backing_index = usize::from(
            *self
                .retention()
                .active_backing_indices
                .get(logical_index)
                .ok_or(AmpduDmaStorageError::Count)?,
        );
        let identity = self
            .retention()
            .backing_identities
            .get(backing_index)
            .copied()
            .flatten()
            .ok_or(AmpduDmaStorageError::StaleBacking)?;
        let offset = address
            .checked_sub(identity.address)
            .ok_or(AmpduDmaStorageError::Address)?;
        let end = offset
            .checked_add(capacity)
            .filter(|end| *end <= identity.len)
            .ok_or(AmpduDmaStorageError::Address)?;
        let bytes = self.retention_mut().backings[backing_index]
            .as_mut()
            .ok_or(AmpduDmaStorageError::StaleBacking)?
            .stable_dma_region()
            .into_mut_slice();
        Ok(&mut bytes[offset..end])
    }

    /// Reorder logical MPDUs after a detached selective BlockAck result.
    #[cfg(any(test, feature = "test-support"))]
    pub fn compact_active_backings(
        &mut self,
        source_indices: &[u8],
    ) -> Result<(), AmpduDmaStorageError> {
        if self.state() != AmpduDmaState::Detached {
            return Err(AmpduDmaStorageError::State);
        }
        if source_indices.is_empty() || source_indices.len() > SLOTS {
            return Err(AmpduDmaStorageError::Count);
        }
        let mut next = self.retention().active_backing_indices;
        for (destination, source) in source_indices.iter().copied().enumerate() {
            let source = usize::from(source);
            if source >= self.active_count
                || source_indices[..destination].contains(&(source as u8))
            {
                return Err(AmpduDmaStorageError::Count);
            }
            next[destination] = self.retention().active_backing_indices[source];
        }
        self.retention_mut().active_backing_indices = next;
        self.active_count = source_indices.len();
        Ok(())
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
        for backing in &mut self.retention_mut().backings[..held] {
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
            if end <= bytes.len() {
                return Ok(&mut bytes[offset..end]);
            }
        }
        Err(AmpduDmaStorageError::Address)
    }

    /// Publish a complete chain whose entries resolve into retained leases.
    pub fn publish_external_chain(
        &mut self,
        entries: &[AmpduExternalDescriptor<'_>],
    ) -> Result<
        RetainedAmpduDmaPublication<'_, 'retention, B, SLOTS, BUFFER_SIZE>,
        AmpduDmaStorageError,
    > {
        if self.state() != AmpduDmaState::Reserved {
            return Err(AmpduDmaStorageError::State);
        }
        if entries.is_empty() || entries.len() > SLOTS {
            return Err(AmpduDmaStorageError::Count);
        }

        // This is a transaction over the descriptor arena: derive every
        // address and ownership word before the first volatile publication.
        let mut words = [WifiTxDmaDescriptorControl::ZERO; SLOTS];
        let mut buffer_addresses = [None; SLOTS];
        let mut descriptor_addresses = [None; SLOTS];
        for (index, entry) in entries.iter().enumerate() {
            let backing = entry.backing.ok_or(AmpduDmaStorageError::StaleBacking)?;
            let identity = self.validate_backing(backing)?;
            let capacity = usize::try_from(entry.buffer_capacity)
                .map_err(|_| AmpduDmaStorageError::Address)?;
            let offset = entry
                .address
                .checked_sub(identity.address)
                .ok_or(AmpduDmaStorageError::Address)?;
            offset
                .checked_add(capacity)
                .filter(|end| *end <= identity.len)
                .ok_or(AmpduDmaStorageError::Address)?;
            buffer_addresses[index] = Some(external_dma_address(identity, offset)?);
            descriptor_addresses[index] = Some(
                self.dma()
                    .binding
                    .descriptor(index)
                    .ok_or(AmpduDmaStorageError::Address)?,
            );
            words[index] = tx_owned_word(entry.buffer_capacity, entry.transfer_length)
                .ok_or(AmpduDmaStorageError::InvalidLength)?;
        }

        // The encoder mutates retained packet bytes after their stable address
        // is sampled. Establish external-memory visibility at the last
        // software-owned edge, before the first descriptor publishes those
        // addresses to the Wi-Fi DMA reader.
        self.prepare_backings_for_dma_read()?;

        for index in 0..entries.len() {
            let next = if index + 1 == entries.len() {
                None
            } else {
                descriptor_addresses[index + 1]
            };
            let word0 = words[index].with_tx_last(next.is_none());
            let buffer = buffer_addresses[index].ok_or(AmpduDmaStorageError::Address)?;
            self.dma().storage.as_ref().get_ref().descriptors[index]
                .publish_owned(word0, buffer, next);
        }
        *self.dma_mut().storage.as_mut().project().published_count = entries.len();

        Ok(RetainedAmpduDmaPublication { owner: self })
    }

    /// Publish descriptor images previously bound to current retained leases.
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
        if count == 0 || count > SLOTS || count != self.active_count {
            return Err(AmpduDmaStorageError::Count);
        }

        let mut descriptors = [None; SLOTS];
        for (logical_index, destination) in descriptors.iter_mut().enumerate().take(count) {
            let backing_index = usize::from(self.retention().active_backing_indices[logical_index]);
            let descriptor = self
                .retention()
                .backing_descriptors
                .get(backing_index)
                .copied()
                .flatten()
                .ok_or(AmpduDmaStorageError::StaleBacking)?;
            self.dma()
                .binding
                .descriptor_address(logical_index)
                .ok_or(AmpduDmaStorageError::Address)?;
            *destination = Some(descriptor);
        }

        // Retry compaction can also mutate retained metadata, so every
        // publication (not only the first one) must re-establish visibility.
        self.prepare_backings_for_dma_read()?;

        for (logical_index, descriptor) in descriptors[..count].iter().copied().enumerate() {
            let descriptor = descriptor.ok_or(AmpduDmaStorageError::StaleBacking)?;
            let next = if logical_index + 1 == count {
                None
            } else {
                Some(
                    self.dma()
                        .binding
                        .descriptor(logical_index + 1)
                        .ok_or(AmpduDmaStorageError::Address)?,
                )
            };
            let word0 = descriptor.word0.with_tx_last(next.is_none());
            let buffer = descriptor.buffer_address;
            self.dma().storage.as_ref().get_ref().descriptors[logical_index]
                .publish_owned(word0, buffer, next);
        }
        *self.dma_mut().storage.as_mut().project().published_count = count;

        Ok(RetainedAmpduDmaPublication { owner: self })
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
    owner: &'owner mut RetainedAmpduDma<'retention, B, SLOTS, BUFFER_SIZE>,
}

#[allow(
    unsafe_code,
    reason = "publication token is coupled to the owner retaining every external lease"
)]
// SAFETY: this token exists only after the retaining owner published the
// complete aggregate chain; it mutably borrows that owner, which retains
// every external lease, and hardware cannot own the chain before `commit`.
unsafe impl<B: PlacedBacking, const SLOTS: usize, const BUFFER_SIZE: usize>
    PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>
    for RetainedAmpduDmaPublication<'_, '_, B, SLOTS, BUFFER_SIZE>
{
    fn descriptor_head(&self) -> DmaRef<WifiTxDmaDescriptor> {
        self.owner.dma().binding.descriptors.first()
    }
}

impl<B, const SLOTS: usize, const BUFFER_SIZE: usize>
    RetainedAmpduDmaPublication<'_, '_, B, SLOTS, BUFFER_SIZE>
{
    pub fn commit<F>(self, start: F)
    where
        F: FnOnce(&AmpduDmaStart<'_>),
    {
        let owner = self.owner;
        let dma = owner.dma_mut();
        *dma.storage.as_mut().project().state = AmpduDmaState::HardwareOwned;
        start(&AmpduDmaStart {
            binding: &dma.binding,
        });
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
