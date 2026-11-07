#![no_std]
use oer_chips_esp32s31_radio_ieee80211_backend::mac::irq::epoch::{InterruptEpoch, Moderated};
pub fn close(epoch: &mut InterruptEpoch<Moderated>) {
    #[cfg(not(ui_reject))]
    let _ = epoch.mask_all().clear_all();
    #[cfg(ui_reject)]
    epoch.clear_all(); // ui-error: E0599
}
