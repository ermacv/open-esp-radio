#![no_std]
use oer_protocols_ieee80211_core_security_rsn::{Pmk, sae::SaeKeys};
pub fn borrow(keys: &SaeKeys) -> &Pmk {
    #[cfg(ui_reject)]
    let bytes = &keys.pmk; // ui-error: E0616
    #[cfg(not(ui_reject))]
    let bytes = keys.pmk();
    bytes
}
