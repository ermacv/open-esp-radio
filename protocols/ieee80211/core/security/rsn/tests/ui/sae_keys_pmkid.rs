#![no_std]
use oer_protocols_ieee80211_core_security_rsn::sae::SaeKeys;
pub fn borrow(keys: &SaeKeys) {
    #[cfg(ui_reject)]
    let bytes = &keys.pmkid; // ui-error: E0616
    #[cfg(not(ui_reject))]
    let bytes = keys.pmkid();
    let _ = bytes;
}
