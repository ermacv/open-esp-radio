//! Pinned ordinary-TX descriptor and buffer ownership.
//!
//! This module owns only DMA memory and its publication lifecycle. Queue
//! selection, PLCP/rate formatting, retry policy and completion decoding stay
//! in the MAC backend. The split lets those safe upper layers pass an unforgeable memory
//! capability instead of a bare descriptor address to the register boundary.

use crate::memory::dma_static::Placed;
use core::{marker::PhantomPinned, pin::Pin};

use crate::types::MacTxQueueDetached;
use oer_memory::{HardwareOwnedTxDma, PreparedTxDma, StableDmaRange};
use pin_project::pin_project;

use crate::ieee80211::dma::space::DmaSpace;

use crate::layouts::{DmaRef, DmaWindowAddress, WifiTxDmaDescriptor, WifiTxDmaDescriptorControl};

use crate::ieee80211::dma::descriptor::{DESCRIPTOR_BYTES, TxDescriptor, tx_owned_word};

/// DMA-memory state independent of an EDCA queue or protocol cookie.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, bytemuck::Zeroable)]
#[repr(u8)]
pub enum TxDmaState {
    #[default]
    Free,
    Reserved,
    HardwareOwned,
    Completed,
    ResetRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxDmaStorageError {
    Address,
    InvalidLength,
    State,
}

#[repr(C, align(16))]
#[derive(bytemuck::Zeroable)]
struct TxDmaBuffer<const BUFFER_SIZE: usize>([u8; BUFFER_SIZE]);

/// Final allocation containing one ordinary TX descriptor and source buffer.
///
/// Declare it with [`dma_static!`](crate::dma_static!) and consume the placed
/// allocation through [`Self::pin`]. The returned owner is movable; this
/// backing allocation is not.
///
/// Its layout is fixed: the descriptor, then the 16-byte aligned buffer.
#[pin_project]
#[derive(bytemuck::Zeroable)]
#[repr(C)]
pub struct TxDmaStorage<const BUFFER_SIZE: usize> {
    #[pin]
    descriptor: TxDescriptor,
    #[pin]
    buffer: TxDmaBuffer<BUFFER_SIZE>,
    state: TxDmaState,
    #[pin]
    _pin: PhantomPinned,
}

impl<const BUFFER_SIZE: usize> TxDmaStorage<BUFFER_SIZE> {
    pub const fn new() -> Self {
        Self {
            descriptor: TxDescriptor::new(),
            buffer: TxDmaBuffer([0; BUFFER_SIZE]),
            state: TxDmaState::Free,
            _pin: PhantomPinned,
        }
    }

    /// Consume the placed hardware allocation and bind its DMA addresses in
    /// space `S` to a movable owner, once. The link keeps the placed storage
    /// in the DMA window, so its addresses need no window check.
    pub fn pin<S: DmaSpace>(
        placed: Placed<Self>,
    ) -> Result<PinnedTxDmaStorage<BUFFER_SIZE>, TxDmaStorageError> {
        let (storage, range, projection) = placed
            .into_dma_parts::<S>()
            .ok_or(TxDmaStorageError::Address)?;
        let descriptor_head = projection
            .link::<WifiTxDmaDescriptor>(storage.descriptor.layout())
            .map_err(|_| TxDmaStorageError::Address)?;
        let buffer = projection
            .region(&storage.buffer.0)
            .map_err(|_| TxDmaStorageError::Address)?
            .start();
        let descriptor_address = descriptor_head.bits();
        let buffer_address = buffer.get();
        let buffer_len = u32::try_from(BUFFER_SIZE).map_err(|_| TxDmaStorageError::Address)?;

        let descriptor_range = range
            .subrange(descriptor_address, DESCRIPTOR_BYTES)
            .ok_or(TxDmaStorageError::Address)?;
        let buffer_range = range
            .subrange(buffer_address, buffer_len)
            .ok_or(TxDmaStorageError::Address)?;

        Ok(PinnedTxDmaStorage {
            storage: Pin::static_mut(storage),
            binding: TxDmaBinding {
                descriptor_head,
                buffer,
                buffer_capacity: buffer_len,
                descriptor_range,
                buffer_range,
            },
        })
    }
}

impl<const BUFFER_SIZE: usize> Default for TxDmaStorage<BUFFER_SIZE> {
    fn default() -> Self {
        Self::new()
    }
}

/// Non-forgeable memory authority for one ordinary TX descriptor chain.
pub struct TxDmaBinding {
    descriptor_head: DmaRef<WifiTxDmaDescriptor>,
    buffer: DmaWindowAddress,
    buffer_capacity: u32,
    descriptor_range: StableDmaRange<'static>,
    buffer_range: StableDmaRange<'static>,
}

impl TxDmaBinding {
    pub const fn descriptor_address(&self) -> u32 {
        self.descriptor_head.bits()
    }

    pub const fn buffer_address(&self) -> u32 {
        self.buffer.get()
    }

    pub const fn buffer_capacity(&self) -> u32 {
        self.buffer_capacity
    }

    pub const fn admits_descriptor(&self, address: u32) -> bool {
        self.descriptor_range.contains(address, DESCRIPTOR_BYTES)
    }

    pub const fn admits_buffer(&self, address: u32, len: u32) -> bool {
        self.buffer_range.contains(address, len)
    }
}

/// Start-only authority created after software records hardware ownership.
pub struct TxDmaStart<'owner> {
    binding: &'owner TxDmaBinding,
}

impl TxDmaStart<'_> {
    pub const fn binding(&self) -> &TxDmaBinding {
        self.binding
    }
}

#[allow(
    unsafe_code,
    reason = "start token is exposed only after the owner records hardware ownership"
)]
// SAFETY: `commit` records `HardwareOwned` before creating this token and
// only lends it to the start closure; the owner keeps the pinned descriptor
// and buffer until completion or reset, and the private field prevents
// forging the token.
unsafe impl HardwareOwnedTxDma<DmaRef<WifiTxDmaDescriptor>> for TxDmaStart<'_> {
    fn descriptor_head(&self) -> DmaRef<WifiTxDmaDescriptor> {
        self.binding.descriptor_head
    }
}

/// Unique movable owner of one permanently located TX DMA allocation.
///
/// The allocation, descriptor state and quarantine marker outlive this
/// movable capability. Dropping a hardware-owned capability performs no
/// implicit detach and never unwinds; the backing remains unavailable until
/// the queue is explicitly detached or the radio is reset.
pub struct PinnedTxDmaStorage<const BUFFER_SIZE: usize> {
    storage: Pin<&'static mut TxDmaStorage<BUFFER_SIZE>>,
    binding: TxDmaBinding,
}

/// The phase of an exclusively borrowed DMA owner. Only `Free` grants
/// authority to edit the buffer or reserve a descriptor.
pub enum TxDmaAccess<'owner, const BUFFER_SIZE: usize> {
    Free(TxDmaFree<'owner, BUFFER_SIZE>),
    Reserved,
    HardwareOwned,
    Completed,
    ResetRequired,
}

/// Exclusive software authority over one free TX allocation.
///
/// ```compile_fail,E0382
/// use oer_esp32s31_hal::ieee80211::dma::tx_storage::TxDmaFree;
/// fn reserve_twice(free: TxDmaFree<'_, 64>) {
///     let _ = free.reserve(64, 8);
///     let _ = free.reserve(64, 8);
/// }
/// ```
pub struct TxDmaFree<'owner, const BUFFER_SIZE: usize> {
    owner: &'owner mut PinnedTxDmaStorage<BUFFER_SIZE>,
}

impl<const BUFFER_SIZE: usize> PinnedTxDmaStorage<BUFFER_SIZE> {
    pub fn state(&self) -> TxDmaState {
        self.storage.as_ref().get_ref().state
    }

    pub const fn binding(&self) -> &TxDmaBinding {
        &self.binding
    }

    /// Classify and lend the software phase for the whole bounded operation.
    pub fn access(&mut self) -> TxDmaAccess<'_, BUFFER_SIZE> {
        match self.state() {
            TxDmaState::Free => TxDmaAccess::Free(TxDmaFree { owner: self }),
            TxDmaState::Reserved => TxDmaAccess::Reserved,
            TxDmaState::HardwareOwned => TxDmaAccess::HardwareOwned,
            TxDmaState::Completed => TxDmaAccess::Completed,
            TxDmaState::ResetRequired => TxDmaAccess::ResetRequired,
        }
    }

    pub fn descriptor_word0(&self) -> WifiTxDmaDescriptorControl {
        self.storage.as_ref().get_ref().descriptor.word0()
    }

    /// The private transition is callable only through the free loan.
    fn reserve_free(
        &mut self,
        buffer_capacity: u32,
        transfer_length: u32,
    ) -> Result<(), TxDmaStorageError> {
        if buffer_capacity == 0
            || buffer_capacity > self.binding.buffer_capacity
            || transfer_length == 0
            || transfer_length > buffer_capacity
        {
            return Err(TxDmaStorageError::InvalidLength);
        }
        let word0 = tx_owned_word(buffer_capacity, transfer_length)
            .ok_or(TxDmaStorageError::InvalidLength)?;
        let storage = self.storage.as_mut().project();
        storage
            .descriptor
            .as_ref()
            .get_ref()
            .publish_owned(word0, self.binding.buffer, None);
        *storage.state = TxDmaState::Reserved;
        Ok(())
    }

    /// Borrow the prepare-phase capability for one reserved descriptor.
    pub fn publication(&mut self) -> Result<TxDmaPublication<'_, BUFFER_SIZE>, TxDmaStorageError> {
        if self.state() != TxDmaState::Reserved {
            return Err(TxDmaStorageError::State);
        }
        Ok(TxDmaPublication { owner: self })
    }

    pub fn cancel_reservation(&mut self) -> Result<(), TxDmaStorageError> {
        self.transition_to_free(TxDmaState::Reserved)
    }

    pub fn mark_completed(&mut self) -> Result<(), TxDmaStorageError> {
        self.transition(TxDmaState::HardwareOwned, TxDmaState::Completed)
    }

    pub fn release_completed(
        &mut self,
        detached: MacTxQueueDetached<'_>,
    ) -> Result<(), TxDmaStorageError> {
        if !detached.confirms_descriptor_head(self.binding.descriptor_head.bits()) {
            return Err(TxDmaStorageError::Address);
        }
        self.transition_to_free(TxDmaState::Completed)
    }

    pub fn release_aborted(
        &mut self,
        detached: MacTxQueueDetached<'_>,
    ) -> Result<(), TxDmaStorageError> {
        if !detached.confirms_descriptor_head(self.binding.descriptor_head.bits()) {
            return Err(TxDmaStorageError::Address);
        }
        self.transition_to_free(TxDmaState::HardwareOwned)
    }

    pub fn require_reset(&mut self) -> Result<(), TxDmaStorageError> {
        self.transition(TxDmaState::HardwareOwned, TxDmaState::ResetRequired)
    }

    /// Fail closed after an impossible completion sequence or failed detach.
    ///
    /// Quarantine deliberately has no recovery edge. It is valid from every
    /// state because retaining otherwise idle backing is safe, while guessing
    /// that an asynchronously observed queue no longer references it is not.
    pub fn quarantine(&mut self) {
        *self.storage.as_mut().project().state = TxDmaState::ResetRequired;
    }

    fn transition(
        &mut self,
        expected: TxDmaState,
        next: TxDmaState,
    ) -> Result<(), TxDmaStorageError> {
        let storage = self.storage.as_mut().project();
        if *storage.state != expected {
            return Err(TxDmaStorageError::State);
        }
        *storage.state = next;
        Ok(())
    }

    fn transition_to_free(&mut self, expected: TxDmaState) -> Result<(), TxDmaStorageError> {
        let storage = self.storage.as_mut().project();
        if *storage.state != expected {
            return Err(TxDmaStorageError::State);
        }
        storage.descriptor.as_ref().get_ref().clear();
        *storage.state = TxDmaState::Free;
        Ok(())
    }
}

impl<'owner, const BUFFER_SIZE: usize> TxDmaFree<'owner, BUFFER_SIZE> {
    pub fn buffer_mut(&mut self) -> &mut [u8; BUFFER_SIZE] {
        &mut self.owner.storage.as_mut().project().buffer.get_mut().0
    }

    pub fn into_buffer(self) -> &'owner mut [u8; BUFFER_SIZE] {
        &mut self.owner.storage.as_mut().project().buffer.get_mut().0
    }

    /// Consume free authority and publish a software-owned descriptor. A
    /// rejected external length leaves the allocation free.
    pub fn reserve(
        self,
        buffer_capacity: u32,
        transfer_length: u32,
    ) -> Result<(), TxDmaStorageError> {
        self.owner.reserve_free(buffer_capacity, transfer_length)
    }
}

/// Prepare-phase borrow which cannot itself manufacture the final start edge.
pub struct TxDmaPublication<'owner, const BUFFER_SIZE: usize> {
    owner: &'owner mut PinnedTxDmaStorage<BUFFER_SIZE>,
}

impl<const BUFFER_SIZE: usize> TxDmaPublication<'_, BUFFER_SIZE> {
    pub const fn binding(&self) -> &TxDmaBinding {
        &self.owner.binding
    }

    /// Record hardware ownership before invoking the queue doorbell.
    pub fn commit<F>(self, start: F)
    where
        F: FnOnce(&TxDmaStart<'_>),
    {
        let owner = self.owner;
        *owner.storage.as_mut().project().state = TxDmaState::HardwareOwned;
        start(&TxDmaStart {
            binding: &owner.binding,
        });
    }
}

#[allow(
    unsafe_code,
    reason = "publication token retains the initialized software-owned descriptor"
)]
// SAFETY: this token is a `&mut` borrow of a Reserved owner whose pinned
// storage holds the initialized software-owned descriptor and buffer;
// hardware cannot own it before `commit`, and the private field prevents
// independent construction.
unsafe impl<const BUFFER_SIZE: usize> PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>
    for TxDmaPublication<'_, BUFFER_SIZE>
{
    fn descriptor_head(&self) -> DmaRef<WifiTxDmaDescriptor> {
        self.owner.binding.descriptor_head
    }
}

#[cfg(test)]
mod tests;
