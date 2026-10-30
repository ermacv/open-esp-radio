#![no_std]
use oer_ieee80211_core_security_rsn::sae::SaePasswordElement;
pub fn transfer(key: SaePasswordElement) {
    let installed = key;
    #[cfg(ui_reject)]
    let second = key; // ui-error: E0382
    #[cfg(not(ui_reject))]
    let second = installed;
    drop(second);
}
