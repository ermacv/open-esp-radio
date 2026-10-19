#![no_std]
use oer_ieee80211_mac::ccmp::Ccmp128Key;
pub fn import(material: &[u8; 16]) -> Ccmp128Key {
    #[cfg(ui_reject)]
    let key = Ccmp128Key::new(*material); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let key = Ccmp128Key::new(material);
    key
}
