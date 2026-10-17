#![no_std]
use oer_esp32s31_ieee80211::{
    lower_mac::LowerMacCore,
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
};
use oer_ieee80211_lower_mac::BssColor;
use oer_time::Timer;

pub fn install<P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer>(
    core: &mut LowerMacCore<'_, '_, P, E, T, 512>,
    color: BssColor,
) {
    #[cfg(ui_reject)]
    core.set_checked_he_bss_color(color.get()); // ui-error: E0308
    #[cfg(not(ui_reject))]
    core.set_checked_he_bss_color(color);
}
