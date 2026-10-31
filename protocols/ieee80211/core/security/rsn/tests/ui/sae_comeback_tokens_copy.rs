#![no_std]
use oer_ieee80211_core_security_rsn::sae::SaeComebackTokens;
pub fn transfer(key: SaeComebackTokens) {
    let installed = key;
    #[cfg(ui_reject)]
    let second = key; // ui-error: E0382
    #[cfg(not(ui_reject))]
    let second = installed;
    drop(second);
}
