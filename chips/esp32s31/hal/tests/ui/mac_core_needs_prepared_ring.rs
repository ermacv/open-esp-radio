#![no_std]
use oer_esp32s31_hal::ieee80211::{
    dma::{rx_ring::RxRingPrepared, rx_storage::RxDmaStorage},
    mac::WifiMacHal,
};

pub fn prepare(
    mac: &mut WifiMacHal<'_>,
    storage: &RxDmaStorage<2, 16, 20>,
    prepared: RxRingPrepared<'_, 2>,
) {
    #[cfg(ui_reject)]
    mac.prepare_mac_runtime(storage); // ui-error: E0308
    #[cfg(not(ui_reject))]
    mac.prepare_mac_runtime(prepared);
}
