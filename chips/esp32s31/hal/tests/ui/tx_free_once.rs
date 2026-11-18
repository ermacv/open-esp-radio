#![no_std]
use oer_chips_esp32s31_hal::radio::ieee80211::dma::tx_storage::TxDmaFree;
use oer_chips_esp32s31_hal::radio::ieee80211::tx_frame::TxFrame; use oer_chips_esp32s31_hal::system::memory::dma_static::PlacedBacking;

pub fn reserve<L: PlacedBacking>(free: TxDmaFree<'_, L>, frame: TxFrame) {
    let _ = free.reserve(frame);
    #[cfg(ui_reject)]
    let _ = free.reserve(frame); // ui-error: E0382
}
