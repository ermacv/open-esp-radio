#![no_std]
use oer_esp32s31_ieee80211_driver::ordinary_tx::{OrdinaryTxIdle, OrdinaryTxPlan};
use oer_esp32s31_ieee80211_driver::tx::{WifiTxEntropy, WifiTxPowerProfile};
use oer_esp32s31_ieee80211_driver::mac::tx::TxHardware;
use oer_time::Timer;

pub fn start<P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, H: TxHardware>(
    mut idle: OrdinaryTxIdle<'_, '_, P, E, T, 64>,
    hardware: &mut H,
    plan: OrdinaryTxPlan,
) {
    if let Ok(buffer) = idle.buffer_mut() {
        buffer[0] = 1;
    }
    let _ = idle.start_single_attempt(hardware, plan, oer_esp32s31_ieee80211_driver::ordinary_tx::SingleAttempt::new(oer_esp32s31_ieee80211_driver::ordinary_tx::SingleAttemptProtection::None));
    #[cfg(ui_reject)]
    let _ = idle.start_single_attempt(hardware, plan, oer_esp32s31_ieee80211_driver::ordinary_tx::SingleAttempt::new(oer_esp32s31_ieee80211_driver::ordinary_tx::SingleAttemptProtection::None)); // ui-error: E0382
}
