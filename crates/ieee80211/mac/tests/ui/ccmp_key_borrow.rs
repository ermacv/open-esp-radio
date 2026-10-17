#![no_std]
use oer_ieee80211_mac::ccmp::Ccmp128Key;
pub fn programming(key: Ccmp128Key) -> u8 {
    let bytes = key.as_bytes();
    #[cfg(ui_reject)]
    drop(key); // ui-error: E0505
    let first = bytes[0];
    #[cfg(not(ui_reject))]
    drop(key);
    first
}
