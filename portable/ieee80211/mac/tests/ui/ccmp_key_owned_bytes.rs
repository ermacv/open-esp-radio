#![no_std]
use oer_ieee80211_mac::ccmp::Ccmp128Key;
pub fn programming(key: &Ccmp128Key) {
    #[cfg(ui_reject)]
    let _bytes: [u8; 16] = key.as_bytes(); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _bytes: &[u8; 16] = key.as_bytes();
}
