#![no_std]
use oer_ieee80211_rsn::sae::SaePassword;
pub fn transfer(key: SaePassword) {
    let installed = key;
    #[cfg(ui_reject)]
    let second = key; // ui-error: E0382
    #[cfg(not(ui_reject))]
    let second = installed;
    drop(second);
}
