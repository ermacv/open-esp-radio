#![no_std]
use oer_ieee80211_rsn::sae::SaeKeys;
pub fn transfer(key: SaeKeys) {
    let installed = key;
    #[cfg(ui_reject)]
    let second = key; // ui-error: E0382
    #[cfg(not(ui_reject))]
    let second = installed;
    drop(second);
}
