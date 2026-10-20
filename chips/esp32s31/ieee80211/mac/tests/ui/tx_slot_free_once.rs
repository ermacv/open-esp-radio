#![no_std]
use oer_esp32s31_ieee80211_mac::tx::TxSlotFree;

pub fn reserve(mut free: TxSlotFree<'_, 64>) {
    free.buffer_mut()[0] = 1;
    let _ = free.reserve(64, 8);
    #[cfg(ui_reject)]
    let _ = free.reserve(64, 8); // ui-error: E0382
}
