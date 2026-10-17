#![no_std]
use oer_esp32s31_ieee80211::lower_mac::KeyUse;
pub fn duplicate<'core>(key: KeyUse<'core>) -> KeyUse<'core> {
    #[cfg(ui_reject)]
    let _copy = key.clone(); // ui-error: E0599
    key
}
