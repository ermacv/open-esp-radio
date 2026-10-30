#![no_std]
use oer_ieee80211_core_security_rsn::sae::SaeComebackTokens;
pub fn transfer(key: SaeComebackTokens) -> SaeComebackTokens {
    #[cfg(ui_reject)]
    let installed = key.clone(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let installed = key;
    installed
}
