#![no_std]
use oer_esp32s31_hal::ieee80211::dma::rx_ring::RecycleGrant;

pub fn consume(grant: RecycleGrant<'_>) {
    let _index = grant.index();
    drop(grant);
    #[cfg(ui_reject)]
    drop(grant); // ui-error: E0382
}
