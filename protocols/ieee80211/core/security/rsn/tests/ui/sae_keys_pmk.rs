#![no_std]
use oer_ieee80211_core_security_rsn::sae::SaeKeys;
pub fn borrow(keys: &SaeKeys) -> &[u8; 32] {
    #[cfg(ui_reject)]
    let bytes = &keys.pmk; // ui-error: E0616
    #[cfg(not(ui_reject))]
    let bytes = keys.pmk();
    bytes
}
