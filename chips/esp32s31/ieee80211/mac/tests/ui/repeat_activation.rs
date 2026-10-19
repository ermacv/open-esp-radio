#![no_std]
use oer_esp32s31_hal::types::MacInterruptMask;
use oer_esp32s31_ieee80211_mac::irq::epoch::InactiveEpoch;
pub fn activate(inactive: InactiveEpoch) {
    let active = inactive.activate_rx_moderated(MacInterruptMask::STATION);
    #[cfg(ui_reject)]
    let _second = inactive.activate_rx_moderated(MacInterruptMask::STATION); // ui-error: E0382
    let _inactive = active.quiesce();
}
