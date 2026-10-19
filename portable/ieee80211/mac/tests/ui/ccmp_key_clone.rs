#![no_std]
use oer_ieee80211_mac::ccmp::Ccmp128Key;
pub fn transfer(key: Ccmp128Key) -> Ccmp128Key {
    #[cfg(ui_reject)]
    let installed = key.clone(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let installed = key;
    installed
}
