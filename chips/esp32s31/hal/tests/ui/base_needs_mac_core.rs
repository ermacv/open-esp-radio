#![no_std]
use oer_esp32s31_hal::ieee80211::dma::{rx_dma::RxDma, rx_ring::{RxRingPrepared, RxRingCoreReady}};
pub fn publish<M: RxDma>(dma: &mut M, memory: RxRingPrepared<'_, 2>, core: RxRingCoreReady<'_, 2>) {
    #[cfg(ui_reject)]
    let _ = memory.publish(dma); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _ = core.publish(dma);
}
