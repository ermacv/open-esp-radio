#![no_std]
use oer_esp32s31_hal::types::MacInterruptMask;
use oer_esp32s31_ieee80211_mac::irq::epoch::InactiveEpoch;
pub fn activate(inactive: InactiveEpoch, mask: MacInterruptMask) {
    #[cfg(not(ui_reject))]
    let mut active = inactive.activate_rx_moderated(mask);
    #[cfg(ui_reject)]
    let mut active = inactive.activate(mask);
    active.unmask_rx_after_drain(); // ui-error: E0599
}
