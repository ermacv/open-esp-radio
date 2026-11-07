#![no_std]
use oer_chips_esp32s31_radio_ieee80211_driver::{
    lower_mac::LowerMacCore,
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
};
use oer_protocols_ieee80211_contracts_port::BssColor;
use oer_portable_time::Timer;

pub fn install<P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, C: oer_chips_esp32s31_radio_ieee80211_driver::lower_mac::RadioCoexPriorities>(
    core: &mut LowerMacCore<'_, '_, P, E, T, C, 512>,
    color: BssColor,
) {
    #[cfg(ui_reject)]
    core.set_checked_he_bss_color(color.get()); // ui-error: E0308
    #[cfg(not(ui_reject))]
    core.set_checked_he_bss_color(color);
}
