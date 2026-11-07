#![no_std]
use oer_chips_esp32s31_hal::radio::ieee80211::dma::tx_storage::TxDmaFree;
use oer_chips_esp32s31_hal::system::memory::dma_static::PlacedBacking;

pub fn reserve<L: PlacedBacking>(free: TxDmaFree<'_, L>) {
    let _ = free.reserve(64, 8);
    #[cfg(ui_reject)]
    let _ = free.reserve(64, 8); // ui-error: E0382
}
