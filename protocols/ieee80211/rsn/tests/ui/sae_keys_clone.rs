#![no_std]
use oer_ieee80211_rsn::sae::SaeKeys;
pub fn transfer(key: SaeKeys) -> SaeKeys {
    #[cfg(ui_reject)]
    let installed = key.clone(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let installed = key;
    installed
}
