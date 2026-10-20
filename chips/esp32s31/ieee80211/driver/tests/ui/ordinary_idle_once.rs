#![no_std]
use oer_esp32s31_ieee80211_driver::ordinary_tx::{OrdinaryTxIdle, OrdinaryTxPlan};
use oer_esp32s31_ieee80211_driver::tx::{WifiTxEntropy, WifiTxPowerProfile};
use oer_esp32s31_ieee80211_mac::tx::TxHardware;
use oer_time::Timer;

pub fn start<P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer, H: TxHardware>(
    mut idle: OrdinaryTxIdle<'_, '_, oer_espressif_ieee80211_policy::backend::EspressifBackendTxProfile, P, E, T, 64>,
    hardware: &mut H,
    plan: OrdinaryTxPlan,
) {
    idle.buffer_mut()[0] = 1;
    let _ = idle.start(hardware, plan);
    #[cfg(ui_reject)]
    let _ = idle.start(hardware, plan); // ui-error: E0382
}
