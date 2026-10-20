#![no_std]
use oer_ieee80211_rsn::sae::SaeKeys;
pub fn borrow(keys: &SaeKeys) -> &[u8; 16] {
    #[cfg(ui_reject)]
    let bytes = &keys.pmkid; // ui-error: E0616
    #[cfg(not(ui_reject))]
    let bytes = keys.pmkid();
    bytes
}
