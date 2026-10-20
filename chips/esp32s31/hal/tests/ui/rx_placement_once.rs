#![no_std]
use oer_esp32s31_hal::ieee80211::dma::{rx_dma::RxDma, rx_storage::PlacedRxDma};

pub fn prepare<M: RxDma>(placed: PlacedRxDma<2, 16, 20>, mmio: &mut M) {
    let _ = placed.prepare_ring(mmio);
    #[cfg(ui_reject)]
    let _ = placed.prepare_ring(mmio); // ui-error: E0382
}
