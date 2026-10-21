#![no_std]
use oer_esp32s31_hal::ieee80211::dma::{rx_dma::RxDma, rx_storage::{PlacedRxDma, RxDmaStorage}};
pub fn prepare<M: RxDma>(dma: &mut M, storage: &RxDmaStorage<2, 16, 20>, placed: PlacedRxDma<2, 16, 20>) {
    #[cfg(ui_reject)]
    let _ = storage.prepare_ring(dma); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _ = placed.prepare_ring(dma);
}
