#![no_std]
use oer_esp32s31_hal::types::MacInterruptMask;
use oer_esp32s31_ieee80211_mac::irq::epoch::{InterruptEpoch, Moderated};
pub fn close(epoch: &mut InterruptEpoch<Moderated>) {
    let masked = epoch.mask_all();
    #[cfg(ui_reject)]
    let _ = epoch.activate(MacInterruptMask::STATION); // ui-error: E0499
    let _ = masked.clear_all();
    let _ = epoch.activate(MacInterruptMask::STATION);
}
