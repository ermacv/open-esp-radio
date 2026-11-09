//! TEMPORARY hardware experiment: branch `probe/rx-descriptor-rebind` only,
//! never merged into `main`. The product contract stays an immutable
//! descriptor-to-buffer binding for one DMA lifecycle.
//!
//! The experiment re-arms one completed, detached descriptor strictly before
//! the frozen LAST with a spare buffer of a placed [`RxSpareBank`], through
//! the production recycle path: BUFFER, NEXT, CONTROL with OWNER last, then
//! the accepted tail's NEXT, fence, doorbell, fence. Only the published
//! address differs from a fixed-binding re-arm.
//!
//! The binding accounting follows the hardware word: `published` is the
//! buffer a descriptor's BUFFER word names (descriptor validation and every
//! CPU read of a completed frame use it); `next` is the buffer the next
//! re-arm publishes, committed to `published` in the same step that stores
//! the BUFFER word. A buffer rebound away is retired: it is never bound
//! again, never leased and stays with the arena until reset. An arena with
//! a rebind is never rebuilt (`RxDmaStorage::prepare_halted` refuses it).

use core::sync::atomic::{AtomicU8, Ordering};

use oer_portable_memory::OnceValue;

use crate::layouts::DmaWindowAddress;
use crate::radio::ieee80211::dma::rx_ring::{RxDmaBufferAddresses, RxRingError};
use crate::radio::ieee80211::dma::rx_storage::RxDmaBuffer;
use crate::system::memory::dma_address::PlacedAddresses;

/// The spare buffers one experiment arena may bind.
pub const RX_REBIND_SPARES: usize = 64;

/// The largest ring the table covers: the observed mask's capacity.
pub(crate) const RX_REBIND_DESCRIPTORS: usize = 96;

/// The spare buffers of one experiment, placed in the DMA window
/// (`dma_static!`) and attached once to one RX arena
/// (`RxDmaStorage::attach_rebind_spares`).
#[repr(C)]
#[derive(bytemuck::Zeroable)]
#[zeroable(bound = "")]
pub struct RxSpareBank<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> {
    pub(crate) buffers: [RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>; RX_REBIND_SPARES],
    pub(crate) addresses: PlacedAddresses<RX_REBIND_SPARES>,
}

impl<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> RxSpareBank<BUFFER_SIZE, STORAGE_SIZE> {
    /// The spare buffers, for the experiment's value-only observation.
    pub const fn buffers(&self) -> &[RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>; RX_REBIND_SPARES] {
        &self.buffers
    }

    /// The DMA address recorded for spare `spare` when it was attached.
    pub fn address(&self, spare: usize) -> Option<u32> {
        self.addresses
            .bits()
            .get(spare)
            .copied()
            .filter(|a| *a != 0)
    }
}

const UNBOUND: u8 = 0;
const BOUND: u8 = 1;
const RETIRED: u8 = 2;

/// Where a descriptor's binding points.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxBinding {
    /// The buffer the arena was placed with.
    Placed,
    /// Spare `k` of the attached bank.
    Spare(usize),
}

impl RxBinding {
    const fn decode(word: u8) -> Self {
        match word {
            0 => Self::Placed,
            spare => Self::Spare(spare as usize - 1),
        }
    }
}

/// The experiment's binding table, kept in the arena words every ring
/// typestate already borrows. Zero bytes: every descriptor on its placed
/// buffer, no spare attached.
#[derive(bytemuck::Zeroable)]
pub(crate) struct RxRebindTable {
    /// Per descriptor, the binding its BUFFER word names: 0 placed, k + 1
    /// spare k.
    published: [AtomicU8; RX_REBIND_DESCRIPTORS],
    /// Per descriptor, the binding its next re-arm publishes.
    next: [AtomicU8; RX_REBIND_DESCRIPTORS],
    /// Per spare: unbound, bound or retired.
    spare_state: [AtomicU8; RX_REBIND_SPARES],
    /// Per descriptor, whether its placed buffer was rebound away.
    placed_retired: [AtomicU8; RX_REBIND_DESCRIPTORS],
    /// The attached spares' DMA addresses.
    spare_addresses: OnceValue<&'static PlacedAddresses<RX_REBIND_SPARES>>,
}

impl RxRebindTable {
    pub(crate) const fn new() -> Self {
        Self {
            published: [const { AtomicU8::new(0) }; RX_REBIND_DESCRIPTORS],
            next: [const { AtomicU8::new(0) }; RX_REBIND_DESCRIPTORS],
            spare_state: [const { AtomicU8::new(UNBOUND) }; RX_REBIND_SPARES],
            placed_retired: [const { AtomicU8::new(0) }; RX_REBIND_DESCRIPTORS],
            spare_addresses: OnceValue::new(),
        }
    }

    pub(crate) fn attach(
        &self,
        addresses: &'static PlacedAddresses<RX_REBIND_SPARES>,
    ) -> Result<(), RxRingError> {
        self.spare_addresses
            .set(addresses)
            .map_err(|_| RxRingError::ResetRequired)
    }

    fn load(words: &[AtomicU8; RX_REBIND_DESCRIPTORS], index: usize) -> Option<RxBinding> {
        words
            .get(index)
            .map(|word| RxBinding::decode(word.load(Ordering::Acquire)))
    }

    /// The binding descriptor `index`'s BUFFER word names.
    pub(crate) fn published(&self, index: usize) -> Option<RxBinding> {
        Self::load(&self.published, index)
    }

    /// The binding the next re-arm of descriptor `index` publishes.
    pub(crate) fn next(&self, index: usize) -> Option<RxBinding> {
        Self::load(&self.next, index)
    }

    /// The typed address the next re-arm of descriptor `index` publishes.
    pub(crate) fn next_address<const COUNT: usize>(
        &self,
        index: usize,
        placed: &RxDmaBufferAddresses<COUNT>,
    ) -> Option<DmaWindowAddress> {
        match self.next(index)? {
            RxBinding::Placed => placed.get(index),
            RxBinding::Spare(spare) => self.spare_addresses.get()?.get(spare),
        }
    }

    /// The address descriptor `index`'s BUFFER word holds by the accounting.
    pub(crate) fn published_address<const COUNT: usize>(
        &self,
        index: usize,
        placed: &RxDmaBufferAddresses<COUNT>,
    ) -> Option<u32> {
        match self.published(index)? {
            RxBinding::Placed => placed.bits().get(index).copied(),
            RxBinding::Spare(spare) => self.spare_addresses.get()?.bits().get(spare).copied(),
        }
    }

    /// Record that descriptor `index`'s BUFFER word now names its `next`
    /// binding: called in the step that stores that word.
    pub(crate) fn commit(&self, index: usize) {
        if let (Some(published), Some(next)) = (self.published.get(index), self.next.get(index)) {
            published.store(next.load(Ordering::Acquire), Ordering::Release);
        }
    }

    /// Bind the next re-arm of descriptor `index` to unbound spare `spare`,
    /// retiring the buffer its BUFFER word names.
    pub(crate) fn bind_next(&self, index: usize, spare: usize) -> Result<(), RxRingError> {
        let (Some(published), Some(next), Some(state)) = (
            self.published.get(index),
            self.next.get(index),
            self.spare_state.get(spare),
        ) else {
            return Err(RxRingError::Count);
        };
        let spare_word = u8::try_from(spare + 1).map_err(|_| RxRingError::Count)?;
        if self.spare_addresses.get().is_none() {
            return Err(RxRingError::Count);
        }
        let current = published.load(Ordering::Acquire);
        if next.load(Ordering::Acquire) != current {
            // A rebind is already pending for this descriptor.
            return Err(RxRingError::Corrupt);
        }
        if state
            .compare_exchange(UNBOUND, BOUND, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(RxRingError::BufferOwnershipConflict);
        }
        match RxBinding::decode(current) {
            RxBinding::Placed => {
                if let Some(retired) = self.placed_retired.get(index) {
                    retired.store(1, Ordering::Release);
                }
            }
            RxBinding::Spare(old) => {
                if let Some(old) = self.spare_state.get(old) {
                    old.store(RETIRED, Ordering::Release);
                }
            }
        }
        next.store(spare_word, Ordering::Release);
        Ok(())
    }

    /// Whether descriptor `index`'s placed buffer was rebound away.
    pub fn placed_retired(&self, index: usize) -> bool {
        self.placed_retired
            .get(index)
            .is_some_and(|word| word.load(Ordering::Acquire) != 0)
    }

    /// Whether spare `spare` is retired.
    pub fn spare_retired(&self, spare: usize) -> bool {
        self.spare_state
            .get(spare)
            .is_some_and(|word| word.load(Ordering::Acquire) == RETIRED)
    }

    /// Whether spare `spare` was never bound.
    pub fn spare_unbound(&self, spare: usize) -> bool {
        self.spare_state
            .get(spare)
            .is_some_and(|word| word.load(Ordering::Acquire) == UNBOUND)
    }

    /// Whether any descriptor of this arena was ever rebound.
    pub(crate) fn any(&self) -> bool {
        self.spare_state
            .iter()
            .any(|word| word.load(Ordering::Acquire) != UNBOUND)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn table() -> &'static RxRebindTable {
        let table = std::boxed::Box::leak(std::boxed::Box::new(RxRebindTable::new()));
        table
            .attach(std::boxed::Box::leak(std::boxed::Box::new(
                PlacedAddresses::new(),
            )))
            .unwrap();
        table
    }

    #[test]
    fn a_retired_spare_never_binds_again() {
        let table = table();
        table.bind_next(0, 0).unwrap();
        table.commit(0);
        table.bind_next(0, 1).unwrap();
        table.commit(0);
        assert!(table.spare_retired(0));
        assert_eq!(table.published(0), Some(RxBinding::Spare(1)));
        assert_eq!(
            table.bind_next(1, 0),
            Err(RxRingError::BufferOwnershipConflict)
        );
        assert!(table.any());
    }

    #[test]
    fn spares_must_be_attached_before_a_rebind() {
        let table = std::boxed::Box::leak(std::boxed::Box::new(RxRebindTable::new()));
        assert_eq!(table.bind_next(0, 0), Err(RxRingError::Count));
        assert!(!table.any());
    }
}
