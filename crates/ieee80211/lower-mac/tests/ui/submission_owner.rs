#![no_std]
use oer_ieee80211_lower_mac::{InterfaceAttempt, InterfacePhase};
pub fn submit<'k, 'p, 'id: 'k + 'p, I: InterfacePhase<'p, 'id>>(
    phase: &mut I,
    attempt: InterfaceAttempt<'k, 'p, 'id, I>,
) {
    #[cfg(ui_reject)]
    let _ = phase.submit(attempt); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _ = phase.submit(attempt.admit());
}
