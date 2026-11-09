//! TEMPORARY hardware experiment: branch `probe/rx-descriptor-rebind` only,
//! never merged into `main`. The product contract stays an immutable
//! descriptor-to-buffer binding for one DMA lifecycle.
//!
//! An experiment arena is placed with a [`RxSpareBank`]
//! (`RxDmaStorage::place_with_spares`): descriptor `i` starts bound to spare
//! `i`, and the remaining spares are the rebind pool. Every buffer the walker
//! can reach is a spare, an array of four-byte [`DmaBytes`] words the CPU
//! reads only by volatile word copies (no buffer-sized value is ever made)
//! copies; the arena's own ring buffers are never published.
//!
//! A completed, detached descriptor strictly before the frozen LAST may be
//! re-armed with an unbound spare through the production recycle path:
//! BUFFER, NEXT, CONTROL with OWNER last, then the accepted tail's NEXT,
//! fence, doorbell, fence. Only the published address differs from a
//! fixed-binding re-arm.
//!
//! The binding accounting follows the hardware word: `published` is the
//! spare a descriptor's BUFFER word names (descriptor validation and every
//! CPU copy of a completed frame use it); `next` is the spare the next re-arm
//! publishes, committed to `published` in the step that stores the BUFFER
//! word. A spare rebound away is retired: it is never bound again, never
//! leased and stays with the arena until reset. An experiment arena is never
//! rebuilt (`RxDmaStorage::prepare_halted` refuses it).

use core::sync::atomic::{AtomicU8, Ordering};

use oer_portable_memory::{DmaBytes, OnceValue};

use crate::layouts::DmaWindowAddress;
use crate::radio::ieee80211::dma::rx_ring::{RxDmaBufferAddresses, RxRingError};
use crate::system::memory::dma_address::PlacedAddresses;

/// The spares of one experiment arena: the ring's initial bindings first,
/// then the rebind pool.
pub const RX_REBIND_SPARES: usize = 96;

/// The largest ring the table covers: the observed mask's capacity.
pub(crate) const RX_REBIND_DESCRIPTORS: usize = 96;

/// The spare buffers of one experiment, placed in the DMA window
/// (`dma_static!`) and consumed by `RxDmaStorage::place_with_spares`.
#[repr(C)]
#[derive(bytemuck::Zeroable)]
#[zeroable(bound = "")]
pub struct RxSpareBank<const WORDS: usize> {
    pub(crate) buffers: [[DmaBytes<4>; WORDS]; RX_REBIND_SPARES],
    pub(crate) addresses: PlacedAddresses<RX_REBIND_SPARES>,
}

impl<const WORDS: usize> RxSpareBank<WORDS> {
    /// A volatile copy of word `word` of spare `spare`, little-endian; no
    /// reference to DMA memory.
    pub fn read_word(&self, spare: usize, word: usize) -> Option<u32> {
        self.buffers
            .get(spare)?
            .get(word)
            .map(|bytes| u32::from_le_bytes(bytes.read()))
    }

    /// The DMA address recorded for spare `spare` at placement.
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
    /// The arena's own ring buffer: an arena placed without spares.
    Arena,
    /// Spare `k` of the bank.
    Spare(usize),
}

impl RxBinding {
    const fn decode(word: u8) -> Self {
        match word {
            0 => Self::Arena,
            spare => Self::Spare(spare as usize - 1),
        }
    }
}

/// The experiment's binding table, kept in the arena words every ring
/// typestate already borrows. Zero bytes: every descriptor on its arena
/// buffer, no bank.
#[derive(bytemuck::Zeroable)]
pub(crate) struct RxRebindTable {
    /// Per descriptor, the binding its BUFFER word names: 0 arena, k + 1
    /// spare k.
    published: [AtomicU8; RX_REBIND_DESCRIPTORS],
    /// Per descriptor, the binding its next re-arm publishes.
    next: [AtomicU8; RX_REBIND_DESCRIPTORS],
    /// Per spare: unbound, bound or retired.
    spare_state: [AtomicU8; RX_REBIND_SPARES],
    /// The bank's DMA addresses, once placed.
    spare_addresses: OnceValue<&'static PlacedAddresses<RX_REBIND_SPARES>>,
}

impl RxRebindTable {
    pub(crate) const fn new() -> Self {
        Self {
            published: [const { AtomicU8::new(0) }; RX_REBIND_DESCRIPTORS],
            next: [const { AtomicU8::new(0) }; RX_REBIND_DESCRIPTORS],
            spare_state: [const { AtomicU8::new(UNBOUND) }; RX_REBIND_SPARES],
            spare_addresses: OnceValue::new(),
        }
    }

    /// Attach the bank and bind descriptor `i` to spare `i` for every
    /// descriptor of a ring of `count`, before the ring exists.
    pub(crate) fn attach(
        &self,
        addresses: &'static PlacedAddresses<RX_REBIND_SPARES>,
        count: usize,
    ) -> Result<(), RxRingError> {
        if count > RX_REBIND_DESCRIPTORS || count > RX_REBIND_SPARES {
            return Err(RxRingError::Count);
        }
        self.spare_addresses
            .set(addresses)
            .map_err(|_| RxRingError::ResetRequired)?;
        for index in 0..count {
            let word = u8::try_from(index + 1).map_err(|_| RxRingError::Count)?;
            let (Some(published), Some(next), Some(state)) = (
                self.published.get(index),
                self.next.get(index),
                self.spare_state.get(index),
            ) else {
                return Err(RxRingError::Count);
            };
            published.store(word, Ordering::Release);
            next.store(word, Ordering::Release);
            state.store(BOUND, Ordering::Release);
        }
        Ok(())
    }

    /// Whether a bank is attached.
    pub(crate) fn attached(&self) -> bool {
        self.spare_addresses.get().is_some()
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

    fn spare_address(&self, spare: usize) -> Option<DmaWindowAddress> {
        self.spare_addresses.get()?.get(spare)
    }

    /// The typed address the next re-arm of descriptor `index` publishes.
    pub(crate) fn next_address<const COUNT: usize>(
        &self,
        index: usize,
        arena: &RxDmaBufferAddresses<COUNT>,
    ) -> Option<DmaWindowAddress> {
        match self.next(index)? {
            RxBinding::Arena => arena.get(index),
            RxBinding::Spare(spare) => self.spare_address(spare),
        }
    }

    /// The address descriptor `index`'s BUFFER word holds by the accounting.
    pub(crate) fn published_address<const COUNT: usize>(
        &self,
        index: usize,
        arena: &RxDmaBufferAddresses<COUNT>,
    ) -> Option<u32> {
        match self.published(index)? {
            RxBinding::Arena => arena.bits().get(index).copied(),
            RxBinding::Spare(spare) => self.spare_address(spare).map(DmaWindowAddress::get),
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
    /// retiring the spare its BUFFER word names.
    pub(crate) fn bind_next(&self, index: usize, spare: usize) -> Result<(), RxRingError> {
        let (Some(published), Some(next), Some(state)) = (
            self.published.get(index),
            self.next.get(index),
            self.spare_state.get(spare),
        ) else {
            return Err(RxRingError::Count);
        };
        let spare_word = u8::try_from(spare + 1).map_err(|_| RxRingError::Count)?;
        let current = published.load(Ordering::Acquire);
        let RxBinding::Spare(old) = RxBinding::decode(current) else {
            // Only an experiment arena's descriptors are rebound.
            return Err(RxRingError::Count);
        };
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
        if let Some(old) = self.spare_state.get(old) {
            old.store(RETIRED, Ordering::Release);
        }
        next.store(spare_word, Ordering::Release);
        Ok(())
    }

    /// Whether spare `spare` is retired.
    pub(crate) fn spare_retired(&self, spare: usize) -> bool {
        self.spare_state
            .get(spare)
            .is_some_and(|word| word.load(Ordering::Acquire) == RETIRED)
    }

    /// Whether spare `spare` was never bound.
    pub(crate) fn spare_unbound(&self, spare: usize) -> bool {
        self.spare_state
            .get(spare)
            .is_some_and(|word| word.load(Ordering::Acquire) == UNBOUND)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn table() -> &'static RxRebindTable {
        let table = std::boxed::Box::leak(std::boxed::Box::new(RxRebindTable::new()));
        table
            .attach(
                std::boxed::Box::leak(std::boxed::Box::new(PlacedAddresses::new())),
                4,
            )
            .unwrap();
        table
    }

    #[test]
    fn placement_binds_descriptor_i_to_spare_i() {
        let table = table();
        assert_eq!(table.published(3), Some(RxBinding::Spare(3)));
        assert!(!table.spare_unbound(3));
        assert!(table.spare_unbound(4));
    }

    #[test]
    fn a_retired_spare_never_binds_again() {
        let table = table();
        table.bind_next(0, 4).unwrap();
        assert_eq!(table.published(0), Some(RxBinding::Spare(0)));
        table.commit(0);
        assert_eq!(table.published(0), Some(RxBinding::Spare(4)));
        assert!(table.spare_retired(0));
        assert_eq!(
            table.bind_next(1, 0),
            Err(RxRingError::BufferOwnershipConflict)
        );
        assert_eq!(
            table.bind_next(1, 2),
            Err(RxRingError::BufferOwnershipConflict)
        );
    }

    #[test]
    fn an_arena_without_a_bank_never_rebinds() {
        let table = std::boxed::Box::leak(std::boxed::Box::new(RxRebindTable::new()));
        assert_eq!(table.bind_next(0, 0), Err(RxRingError::Count));
        assert!(!table.attached());
    }
}
