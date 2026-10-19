#![no_std]
use oer_esp32s31_hal::ieee80211::dma::tx_storage::TxDmaFree;

pub fn reserve(free: TxDmaFree<'_, 64>) {
    let _ = free.reserve(64, 8);
    #[cfg(ui_reject)]
    let _ = free.reserve(64, 8); // ui-error: E0382
}
