#![no_std]
use oer_ieee80211_lower_mac::{Completed, InFlight, SingleOutcome};

pub fn finish<C, T, M>(
    operation: InFlight<C, T, M>,
    outcome: SingleOutcome<(), ()>,
) -> Completed<C, T, M, SingleOutcome<(), ()>> {
    #[cfg(ui_reject)]
    let terminal = Completed { operation, outcome }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let terminal = operation.complete(outcome);
    terminal
}
