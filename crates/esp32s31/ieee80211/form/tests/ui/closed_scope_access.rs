#![no_std]
use oer_esp32s31_ieee80211_form::scope::ScopeAccess;
pub fn abandon(access: ScopeAccess<'_>) {
    #[cfg(not(ui_reject))]
    let _irq = access.irq();
    access.abandon();
    #[cfg(ui_reject)]
    let _irq = access.irq(); // ui-error: E0382
}
