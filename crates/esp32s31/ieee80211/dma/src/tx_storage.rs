//! Pinned ordinary-TX descriptor and buffer ownership.
//!
//! This module owns only DMA memory and its publication lifecycle. Queue
//! selection, PLCP/rate formatting, retry policy and completion decoding stay
//! in the MAC backend. The split lets those safe upper layers pass an unforgeable memory
//! capability instead of a bare descriptor address to the register boundary.

use core::{marker::PhantomPinned, pin::Pin};

use oer_esp32s31_hal::types::MacTxQueueDetached;
use oer_memory::{HardwareOwnedTxDma, PreparedTxDma, StableDmaRange};
use pin_project::pin_project;

use crate::descriptor::{
    DESCRIPTOR_BYTES, Descriptor, descriptor_address_valid, dma_range_valid, tx_owned_word,
};

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
    Busy,
    InvalidLength,
    State,
}

#[repr(C, align(16))]
#[derive(bytemuck::Zeroable)]
struct TxDmaBuffer<const BUFFER_SIZE: usize>([u8; BUFFER_SIZE]);

/// Final allocation containing one ordinary TX descriptor and source buffer.
///
/// Construct this in static storage. The returned owner is movable; this
/// backing allocation is not.
/// Consume the hardware allocation through [`Self::pin_static`].
#[cfg_attr(
    any(test, feature = "test-support"),
    doc = "Explicit models consume the allocation through [`Self::pin_static_model`]."
)]
#[pin_project]
#[derive(bytemuck::Zeroable)]
pub struct TxDmaStorage<const BUFFER_SIZE: usize> {
    #[pin]
    descriptor: Descriptor,
    #[pin]
    buffer: TxDmaBuffer<BUFFER_SIZE>,
    state: TxDmaState,
    #[pin]
    _pin: PhantomPinned,
}

impl<const BUFFER_SIZE: usize> TxDmaStorage<BUFFER_SIZE> {
    pub const fn new() -> Self {
        Self {
            descriptor: Descriptor::new(),
            buffer: TxDmaBuffer([0; BUFFER_SIZE]),
            state: TxDmaState::Free,
            _pin: PhantomPinned,
        }
    }

    /// Consume the unique static hardware allocation and bind its real DMA
    /// addresses to a movable owner.
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<PinnedTxDmaStorage<BUFFER_SIZE>, TxDmaStorageError> {
        let descriptor_address = u32::try_from(core::ptr::addr_of!(storage.descriptor).addr())
            .map_err(|_| TxDmaStorageError::Address)?;
        let buffer_address = u32::try_from(storage.buffer.0.as_ptr().addr())
            .map_err(|_| TxDmaStorageError::Address)?;
        Self::pin_static_inner(storage, descriptor_address, buffer_address)
    }

    /// Bind deterministic low addresses for a native model with no DMA actor.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        descriptor_address: u32,
        buffer_address: u32,
    ) -> Result<PinnedTxDmaStorage<BUFFER_SIZE>, TxDmaStorageError> {
        Self::pin_static_inner(storage, descriptor_address, buffer_address)
    }

    #[allow(
        unsafe_code,
        reason = "static storage is converted once into non-forgeable DMA ranges"
    )]
    fn pin_static_inner(
        storage: &'static mut Self,
        descriptor_address: u32,
        buffer_address: u32,
    ) -> Result<PinnedTxDmaStorage<BUFFER_SIZE>, TxDmaStorageError> {
        let buffer_len = u32::try_from(BUFFER_SIZE).map_err(|_| TxDmaStorageError::Address)?;
        if !descriptor_address_valid(descriptor_address)
            || !dma_range_valid(buffer_address, buffer_len)
        {
            return Err(TxDmaStorageError::Address);
        }

        // SAFETY: `storage` is a consumed unique static allocation. The
        // returned owner retains it permanently, and its state machine is the
        // only safe route to descriptor/buffer mutation.
        let descriptor_range = unsafe {
            StableDmaRange::from_raw_parts(descriptor_address, DESCRIPTOR_BYTES)
                .ok_or(TxDmaStorageError::Address)?
        };
        // SAFETY: the same static owner contains this complete source buffer.
        let buffer_range = unsafe {
            StableDmaRange::from_raw_parts(buffer_address, buffer_len)
                .ok_or(TxDmaStorageError::Address)?
        };

        Ok(PinnedTxDmaStorage {
            storage: Pin::static_mut(storage),
            binding: TxDmaBinding {
                descriptor_address,
                buffer_address,
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
    descriptor_address: u32,
    buffer_address: u32,
    buffer_capacity: u32,
    descriptor_range: StableDmaRange<'static>,
    buffer_range: StableDmaRange<'static>,
}

impl TxDmaBinding {
    pub const fn descriptor_address(&self) -> u32 {
        self.descriptor_address
    }

    pub const fn buffer_address(&self) -> u32 {
        self.buffer_address
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
unsafe impl HardwareOwnedTxDma for TxDmaStart<'_> {
    fn descriptor_head(&self) -> u32 {
        self.binding.descriptor_address
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

impl<const BUFFER_SIZE: usize> PinnedTxDmaStorage<BUFFER_SIZE> {
    pub fn state(&self) -> TxDmaState {
        self.storage.as_ref().get_ref().state
    }

    pub const fn binding(&self) -> &TxDmaBinding {
        &self.binding
    }

    /// Borrow the source buffer only before a descriptor is reserved.
    pub fn buffer_mut(&mut self) -> Result<&mut [u8; BUFFER_SIZE], TxDmaStorageError> {
        if self.state() != TxDmaState::Free {
            return Err(TxDmaStorageError::Busy);
        }
        let storage = self.storage.as_mut().project();
        Ok(&mut storage.buffer.get_mut().0)
    }

    pub fn descriptor_word0(&self) -> u32 {
        self.storage.as_ref().get_ref().descriptor.word0()
    }

    /// Publish a software-owned descriptor without opening a hardware queue.
    pub fn reserve(
        &mut self,
        buffer_capacity: u32,
        transfer_length: u32,
    ) -> Result<(), TxDmaStorageError> {
        if self.state() != TxDmaState::Free {
            return Err(TxDmaStorageError::Busy);
        }
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
            .publish_owned(word0, self.binding.buffer_address, 0);
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
        if !detached.confirms_descriptor_head(self.binding.descriptor_address) {
            return Err(TxDmaStorageError::Address);
        }
        self.transition_to_free(TxDmaState::Completed)
    }

    pub fn release_aborted(
        &mut self,
        detached: MacTxQueueDetached<'_>,
    ) -> Result<(), TxDmaStorageError> {
        if !detached.confirms_descriptor_head(self.binding.descriptor_address) {
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
        storage.descriptor.as_ref().get_ref().publish_owned(0, 0, 0);
        *storage.state = TxDmaState::Free;
        Ok(())
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
unsafe impl<const BUFFER_SIZE: usize> PreparedTxDma for TxDmaPublication<'_, BUFFER_SIZE> {
    fn descriptor_head(&self) -> u32 {
        self.owner.binding.descriptor_address
    }
}

#[cfg(test)]
mod tests;
