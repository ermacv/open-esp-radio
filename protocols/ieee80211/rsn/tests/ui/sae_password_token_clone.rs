#![no_std]
use oer_ieee80211_rsn::sae::SaePasswordToken;
pub fn transfer(key: SaePasswordToken) -> SaePasswordToken {
    #[cfg(ui_reject)]
    let installed = key.clone(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let installed = key;
    installed
}
