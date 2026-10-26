#![no_std]
use oer_esp32s31_hal::ieee80211::dma::rx_ring::RecycleGrant;

pub fn observe(grant: RecycleGrant<'_>) -> usize {
    #[cfg(ui_reject)]
    let _ = RecycleGrant::new(0); // ui-error: E0624
    grant.index()
}
