#![no_std]
use oer_esp32s31_ieee80211_driver::mac::tx::TxSlotFree;

pub fn reserve(mut free: TxSlotFree<'_, 64>) {
    if let Ok(buffer) = free.buffer_mut() {
        buffer[0] = 1;
    }
    let _ = free.reserve(64, 8);
    #[cfg(ui_reject)]
    let _ = free.reserve(64, 8); // ui-error: E0382
}
