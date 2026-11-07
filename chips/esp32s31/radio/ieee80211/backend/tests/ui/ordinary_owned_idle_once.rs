#![no_std]
use oer_chips_esp32s31_radio_ieee80211_backend::ordinary_tx::OrdinaryTxOwner;
use oer_chips_esp32s31_radio_ieee80211_backend::tx::{WifiTxEntropy, WifiTxPowerProfile};
use oer_portable_time::Timer;

pub fn handoff<P: WifiTxPowerProfile, E: WifiTxEntropy, T: Timer>(
    mut owner: OrdinaryTxOwner<'_, P, E, T, 64>,
) {
    let idle = owner.into_idle();
    #[cfg(ui_reject)]
    let _ = owner.access(); // ui-error: E0382
    let _ = idle;
}
