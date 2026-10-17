#![no_std]
use oer_ieee80211_mac::ccmp::Ccmp128Key;
pub fn transfer(key: Ccmp128Key) {
    let installed = key;
    #[cfg(ui_reject)]
    let second = key; // ui-error: E0382
    #[cfg(not(ui_reject))]
    let second = installed;
    drop(second);
}
