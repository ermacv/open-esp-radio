#![no_std]
use oer_protocols_ieee80211_core_security_rsn::sae::SaeKeys;
pub fn borrow(keys: &SaeKeys) -> [u8; 34] {
    #[cfg(ui_reject)]
    let _ = &keys.kck; // ui-error: E0616
    // The KCK has no reader: it keys the Confirm alone.
    keys.own_confirm(1)
}
