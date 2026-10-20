#![no_std]
use oer_esp32s31_ieee80211_driver::ordinary_tx::OrdinaryTxOwner;
use oer_esp32s31_ieee80211_driver::tx::{WifiTxEntropy, WifiTxPowerProfile};
use oer_time::Timer;

pub fn handoff<P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer>(
    mut owner: OrdinaryTxOwner<'_, oer_espressif_ieee80211_policy::backend::EspressifBackendTxProfile, P, E, T, 64>,
) {
    let idle = owner.into_idle();
    #[cfg(ui_reject)]
    let _ = owner.access(); // ui-error: E0382
    let _ = idle;
}
