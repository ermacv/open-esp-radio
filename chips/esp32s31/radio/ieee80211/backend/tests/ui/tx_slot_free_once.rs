#![no_std]
use oer_chips_esp32s31_hal::radio::ieee80211::tx_frame::TxFrame; use oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::TxSlotFree;

pub fn reserve(mut free: TxSlotFree<'_, 64>, frame: TxFrame) {
    if let Ok(buffer) = free.buffer_mut() {
        buffer[0] = 1;
    }
    let _ = free.reserve(frame);
    #[cfg(ui_reject)]
    let _ = free.reserve(frame); // ui-error: E0382
}
