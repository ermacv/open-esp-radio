#![no_std]
use oer_esp32s31_hal::ieee80211::dma::tx_storage::{PinnedTxDmaStorage, TxDmaAccess};
use oer_esp32s31_hal::memory::dma_static::PlacedBacking;

pub fn encode<L: PlacedBacking>(owner: &mut PinnedTxDmaStorage<L>) {
    if let TxDmaAccess::Free(mut free) = owner.access() {
        #[cfg(ui_reject)]
        let _second = owner.access(); // ui-error: E0499
        let _ = free.buffer_mut();
        let _ = free.reserve(64, 8);
    }
}
