//! Pinned ordinary-TX descriptor ownership over an external buffer.
//!
//! This module owns only the descriptor, its publication lifecycle and the
//! buffer it publishes. The buffer is a [`PlacedBacking`] lease the caller
//! attaches while the descriptor is free; the owner retains it until the
//! descriptor is free again and never releases it while hardware may still
//! read it. Queue selection, PLCP/rate formatting, retry policy and
//! completion decoding stay in the MAC backend. The split lets those safe
//! upper layers pass an unforgeable memory capability instead of a bare
//! descriptor address to the register boundary.

use crate::memory::dma_address::PlacedRegion;
use crate::memory::dma_static::{Placed, PlacedBacking};
use core::{marker::PhantomPinned, mem::ManuallyDrop, pin::Pin};

use crate::types::MacTxQueueDetached;
use oer_memory::{HardwareOwnedTxDma, PreparedTxDma, StableDmaRange};
use pin_project::pin_project;

use crate::ieee80211::dma::space::DmaSpace;

use crate::layouts::{DmaRef, WifiTxDmaDescriptor, WifiTxDmaDescriptorControl};

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
    /// No buffer is attached.
    Backing,
}

/// Final allocation containing one ordinary TX descriptor.
///
/// Declare it with [`dma_static!`](crate::dma_static!) and consume the placed
/// allocation through [`Self::pin`]. The returned owner is movable; this
/// backing allocation is not. The buffer the descriptor publishes is a
/// separate placed lease ([`PinnedTxDmaStorage::attach`]).
#[pin_project]
#[derive(bytemuck::Zeroable)]
#[repr(C)]
pub struct TxDmaStorage {
    #[pin]
    descriptor: TxDescriptor,
    state: TxDmaState,
    #[pin]
    _pin: PhantomPinned,
}

impl TxDmaStorage {
    pub const fn new() -> Self {
        Self {
            descriptor: TxDescriptor::new(),
            state: TxDmaState::Free,
            _pin: PhantomPinned,
        }
    }

    /// Consume the placed hardware allocation and bind its DMA address in
    /// space `S` to a movable owner of `L` leases, once. The link keeps the
    /// placed storage in the DMA window, so its address needs no window
    /// check.
    pub fn pin<S: DmaSpace, L>(
        placed: Placed<Self>,
    ) -> Result<PinnedTxDmaStorage<L>, TxDmaStorageError> {
        let (storage, range, projection) = placed
            .into_dma_parts::<S>()
            .ok_or(TxDmaStorageError::Address)?;
        let descriptor_head = projection
            .link::<WifiTxDmaDescriptor>(storage.descriptor.layout())
            .map_err(|_| TxDmaStorageError::Address)?;
        let descriptor_range = range
            .subrange(descriptor_head.bits(), DESCRIPTOR_BYTES)
            .ok_or(TxDmaStorageError::Address)?;
        Ok(PinnedTxDmaStorage {
            storage: Pin::static_mut(storage),
            binding: TxDmaBinding {
                descriptor_head,
                descriptor_range,
            },
            backing: None,
        })
    }
}

impl Default for TxDmaStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Non-forgeable memory authority for one ordinary TX descriptor.
pub struct TxDmaBinding {
    descriptor_head: DmaRef<WifiTxDmaDescriptor>,
    descriptor_range: StableDmaRange<'static>,
}

impl TxDmaBinding {
    pub const fn descriptor_address(&self) -> u32 {
        self.descriptor_head.bits()
    }

    pub const fn admits_descriptor(&self, address: u32) -> bool {
        self.descriptor_range.contains(address, DESCRIPTOR_BYTES)
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
// and its attached buffer lease until completion or reset (its destructor
// leaks a lease the hardware may still read), and the private field
// prevents forging the token.
unsafe impl HardwareOwnedTxDma<DmaRef<WifiTxDmaDescriptor>> for TxDmaStart<'_> {
    fn descriptor_head(&self) -> DmaRef<WifiTxDmaDescriptor> {
        self.binding.descriptor_head
    }
}

/// An attached buffer lease and the DMA region its placement proved.
struct Attached<L> {
    lease: ManuallyDrop<L>,
    region: PlacedRegion,
}

/// Unique movable owner of one permanently located TX descriptor and the
/// buffer lease attached to it.
///
/// The allocation, descriptor state and quarantine marker outlive this
/// movable capability. Dropping a hardware-owned capability performs no
/// implicit detach and never unwinds; the descriptor remains unavailable
/// until the queue is explicitly detached or the radio is reset, and the
/// attached lease is leaked, never returned, since the hardware may still
/// read its buffer.
pub struct PinnedTxDmaStorage<L> {
    storage: Pin<&'static mut TxDmaStorage>,
    binding: TxDmaBinding,
    backing: Option<Attached<L>>,
}

/// The phase of an exclusively borrowed DMA owner. Only `Free` grants
/// authority to edit the buffer or reserve a descriptor.
pub enum TxDmaAccess<'owner, L> {
    Free(TxDmaFree<'owner, L>),
    Reserved,
    HardwareOwned,
    Completed,
    ResetRequired,
}

/// Exclusive software authority over one free TX descriptor and its buffer.
///
/// ```compile_fail,E0382
/// use oer_esp32s31_hal::ieee80211::dma::tx_storage::TxDmaFree;
/// use oer_esp32s31_hal::memory::dma_static::PlacedBacking;
/// fn reserve_twice<L: PlacedBacking>(free: TxDmaFree<'_, L>) {
///     let _ = free.reserve(64, 8);
///     let _ = free.reserve(64, 8);
/// }
/// ```
pub struct TxDmaFree<'owner, L> {
    owner: &'owner mut PinnedTxDmaStorage<L>,
}

impl<L> PinnedTxDmaStorage<L> {
    pub fn state(&self) -> TxDmaState {
        self.storage.as_ref().get_ref().state
    }

    pub const fn binding(&self) -> &TxDmaBinding {
        &self.binding
    }

    /// Whether a buffer lease is attached.
    pub const fn attached(&self) -> bool {
        self.backing.is_some()
    }

    /// Classify and lend the software phase for the whole bounded operation.
    pub fn access(&mut self) -> TxDmaAccess<'_, L> {
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

    /// Take the attached lease back while the descriptor is free; `None`
    /// otherwise, or when none is attached.
    pub fn detach(&mut self) -> Option<L> {
        if self.state() != TxDmaState::Free {
            return None;
        }
        self.backing
            .take()
            .map(|attached| ManuallyDrop::into_inner(attached.lease))
    }

    /// Borrow the prepare-phase capability for one reserved descriptor.
    pub fn publication(&mut self) -> Result<TxDmaPublication<'_, L>, TxDmaStorageError> {
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

impl<L: PlacedBacking> PinnedTxDmaStorage<L> {
    /// Attach the buffer the descriptor publishes, while the descriptor is
    /// free and holds none. A lease without a proved, nonempty DMA region
    /// is refused unchanged.
    pub fn attach(&mut self, backing: L) -> Result<(), L> {
        if self.state() != TxDmaState::Free || self.backing.is_some() {
            return Err(backing);
        }
        match backing.dma_region() {
            Ok(region) if !region.is_empty() => {
                self.backing = Some(Attached {
                    lease: ManuallyDrop::new(backing),
                    region,
                });
                Ok(())
            }
            _ => Err(backing),
        }
    }

    /// The private transition is callable only through the free loan.
    fn reserve_free(
        &mut self,
        buffer_capacity: u32,
        transfer_length: u32,
    ) -> Result<(), TxDmaStorageError> {
        let Some(attached) = self.backing.as_mut() else {
            return Err(TxDmaStorageError::Backing);
        };
        if buffer_capacity == 0
            || buffer_capacity > attached.region.len()
            || transfer_length == 0
            || transfer_length > buffer_capacity
        {
            return Err(TxDmaStorageError::InvalidLength);
        }
        let word0 = tx_owned_word(buffer_capacity, transfer_length)
            .ok_or(TxDmaStorageError::InvalidLength)?;
        // The CPU's last write to the buffer precedes this edge: make it
        // visible before the descriptor names the buffer.
        attached.lease.prepare_for_dma_read();
        let start = attached.region.start();
        let storage = self.storage.as_mut().project();
        storage
            .descriptor
            .as_ref()
            .get_ref()
            .publish_owned(word0, start, None);
        *storage.state = TxDmaState::Reserved;
        Ok(())
    }
}

impl<L> Drop for PinnedTxDmaStorage<L> {
    fn drop(&mut self) {
        let Some(attached) = self.backing.take() else {
            return;
        };
        match self.state() {
            // No queue holds the descriptor: the lease returns.
            TxDmaState::Free | TxDmaState::Reserved => {
                drop(ManuallyDrop::into_inner(attached.lease));
            }
            // The hardware may still read the buffer: the lease is leaked,
            // its allocation never reused.
            TxDmaState::HardwareOwned | TxDmaState::Completed | TxDmaState::ResetRequired => {
                let _leaked = attached;
            }
        }
    }
}

impl<'owner, L: PlacedBacking> TxDmaFree<'owner, L> {
    /// The attached buffer; `Backing` without one.
    pub fn buffer_mut(&mut self) -> Result<&mut [u8], TxDmaStorageError> {
        self.owner
            .backing
            .as_mut()
            .map(|attached| attached.lease.stable_dma_region().into_mut_slice())
            .ok_or(TxDmaStorageError::Backing)
    }

    /// The attached buffer for the rest of the loan; `Backing` without one.
    pub fn into_buffer(self) -> Result<&'owner mut [u8], TxDmaStorageError> {
        self.owner
            .backing
            .as_mut()
            .map(|attached| attached.lease.stable_dma_region().into_mut_slice())
            .ok_or(TxDmaStorageError::Backing)
    }

    /// Consume free authority and publish a software-owned descriptor. A
    /// rejected external length, or no attached buffer, leaves the
    /// allocation free.
    pub fn reserve(
        self,
        buffer_capacity: u32,
        transfer_length: u32,
    ) -> Result<(), TxDmaStorageError> {
        self.owner.reserve_free(buffer_capacity, transfer_length)
    }
}

/// Prepare-phase borrow which cannot itself manufacture the final start edge.
pub struct TxDmaPublication<'owner, L> {
    owner: &'owner mut PinnedTxDmaStorage<L>,
}

impl<L> TxDmaPublication<'_, L> {
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
// storage holds the initialized software-owned descriptor; only reserve
// writes it, from the attached placed lease that the owner retains until
// the descriptor is free again (its destructor leaks the lease otherwise).
// Hardware cannot own it before `commit`, and the private field prevents
// independent construction.
unsafe impl<L> PreparedTxDma<DmaRef<WifiTxDmaDescriptor>> for TxDmaPublication<'_, L> {
    fn descriptor_head(&self) -> DmaRef<WifiTxDmaDescriptor> {
        self.owner.binding.descriptor_head
    }
}

#[cfg(test)]
mod tests;
