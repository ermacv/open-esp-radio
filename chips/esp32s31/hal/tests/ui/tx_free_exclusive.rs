#![no_std]
use oer_esp32s31_hal::ieee80211::dma::tx_storage::{PinnedTxDmaStorage, TxDmaAccess};

pub fn encode(owner: &mut PinnedTxDmaStorage<64>) {
    if let TxDmaAccess::Free(mut free) = owner.access() {
        #[cfg(ui_reject)]
        let _second = owner.access(); // ui-error: E0499
        free.buffer_mut()[0] = 1;
        let _ = free.reserve(64, 8);
    }
}
