#![no_std]
use oer_esp32s31_hal::ieee80211::dma::{rx_dma::RxDma, rx_ring::{RxRingCoreReady, RxRingStopped}};
pub fn start<M: RxDma>(dma: &mut M, core: RxRingCoreReady<'_, 2>, published: RxRingStopped<'_, 2>) {
    #[cfg(ui_reject)]
    let _ = core.try_start(dma); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _ = published.try_start(dma);
}
