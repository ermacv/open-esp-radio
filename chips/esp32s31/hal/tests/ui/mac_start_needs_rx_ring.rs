#![no_std]
use oer_esp32s31_hal::ieee80211::{
    dma::{rx_ring::RxRingLive, rx_storage::RxDmaStorage},
    mac::WifiMacHal,
};

pub fn start(
    mac: &mut WifiMacHal<'_>,
    storage: &RxDmaStorage<2, 16, 20>,
    live: &RxRingLive<'_, 2>,
) {
    #[cfg(ui_reject)]
    mac.start_mac_runtime(storage); // ui-error: E0308
    #[cfg(not(ui_reject))]
    mac.start_mac_runtime(live);
}
