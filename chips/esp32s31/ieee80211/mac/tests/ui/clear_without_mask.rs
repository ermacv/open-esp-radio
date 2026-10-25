#![no_std]
use oer_esp32s31_ieee80211_mac::irq::epoch::{InterruptEpoch, Moderated};
pub fn close(epoch: &mut InterruptEpoch<Moderated>) {
    #[cfg(not(ui_reject))]
    epoch.mask_all().clear_all();
    #[cfg(ui_reject)]
    epoch.clear_all(); // ui-error: E0599
}
