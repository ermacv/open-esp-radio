#![no_std]
use oer_chips_esp32s31_hal::radio::ieee80211::{
    dma::rx_ring::{RxRingLive, RxRingStopped},
    mac::WifiMacHal,
};

pub fn reopen(
    mac: &mut WifiMacHal<'_>,
    stopped: &RxRingStopped<'_, 2>,
    live: &RxRingLive<'_, 2>,
) {
    #[cfg(ui_reject)]
    mac.reopen_mac_core(live); // ui-error: E0308
    #[cfg(not(ui_reject))]
    mac.reopen_mac_core(stopped);
}
