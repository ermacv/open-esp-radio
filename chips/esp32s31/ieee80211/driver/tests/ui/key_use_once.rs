#![no_std]
use oer_esp32s31_ieee80211_driver::lower_mac::{KeyUse, TxAdmission};
pub fn protected<'core>(key: KeyUse<'core>) -> TxAdmission<'core> {
    let admission = TxAdmission::Protected(key);
    #[cfg(ui_reject)]
    let _second_attempt = TxAdmission::Protected(key); // ui-error: E0382
    admission
}
