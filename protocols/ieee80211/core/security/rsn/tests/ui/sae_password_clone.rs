#![no_std]
use oer_ieee80211_core_security_rsn::sae::SaePassword;
pub fn transfer(key: SaePassword) -> SaePassword {
    #[cfg(ui_reject)]
    let installed = key.clone(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let installed = key;
    installed
}
