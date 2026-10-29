#![no_std]
use oer_esp32s31_ieee80211_driver::ordinary_tx::{OrdinaryTxActive, OrdinaryTxPlan};
use oer_esp32s31_ieee80211_driver::tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxWake};
use oer_esp32s31_ieee80211_driver::mac::tx::TxHardware;
use oer_time::Timer;

pub fn service<P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, H: TxHardware>(
    active: OrdinaryTxActive<'_, '_, P, E, T, 64>,
    hardware: &mut H,
    wake: WifiTxWake,
    _plan: OrdinaryTxPlan,
) {
    #[cfg(ui_reject)]
    let _ = active.start_single_attempt(hardware, _plan, oer_esp32s31_ieee80211_driver::ordinary_tx::SingleAttempt::new(oer_esp32s31_ieee80211_driver::ordinary_tx::SingleAttemptProtection::None)); // ui-error: E0599
    let _ = active.service(hardware, wake);
}
