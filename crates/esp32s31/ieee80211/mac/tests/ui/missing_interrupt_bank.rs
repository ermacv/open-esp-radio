#![no_std]
use oer_esp32s31_hal::{owner::MacInterruptSetup, types::MacInterruptMask};
use oer_esp32s31_ieee80211_mac::irq::epoch::InactiveEpoch;
pub fn detach(setup: MacInterruptSetup) {
    let (mac, power) = setup.activate(MacInterruptMask::COLD_RX);
    #[cfg(ui_reject)]
    let _inactive = InactiveEpoch::new(mac); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _inactive = InactiveEpoch::new(mac.deactivate(power));
}
