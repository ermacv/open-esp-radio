#![no_std]
use oer_ieee80211_lower_mac::{InFlight, SingleOutcome};
pub fn finish<C, T, M>(operation: InFlight<C, T, M>, outcome: SingleOutcome<(), ()>) {
    let terminal = operation.complete(outcome);
    #[cfg(ui_reject)]
    let _again = operation.complete(outcome); // ui-error: E0382
    drop(terminal);
}
