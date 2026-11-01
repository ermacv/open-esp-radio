#![no_std]
use oer_esp32s31_hal::ieee80211::dma::tx_storage::TxDmaFree;
use oer_esp32s31_hal::memory::dma_static::PlacedBacking;

pub fn reserve<L: PlacedBacking>(free: TxDmaFree<'_, L>) {
    let _ = free.reserve(64, 8);
    #[cfg(ui_reject)]
    let _ = free.reserve(64, 8); // ui-error: E0382
}
