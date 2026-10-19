#![no_std]
use oer_ieee80211_mac::ccmp::Ccmp128Key;
pub fn programming(key: &Ccmp128Key) -> &[u8; 16] {
    #[cfg(ui_reject)]
    let bytes = &key.bytes; // ui-error: E0616
    #[cfg(not(ui_reject))]
    let bytes = key.as_bytes();
    bytes
}
