#![no_std]
use oer_esp32s31_ieee80211_form::scope::ScopePoisoned;
pub fn reset_required(poisoned: ScopePoisoned) {
    #[cfg(ui_reject)]
    let _stopped = poisoned.into_stopped(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _reset_receipt = poisoned;
}
