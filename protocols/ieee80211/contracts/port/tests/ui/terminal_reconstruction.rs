#![no_std]
use oer_protocols_ieee80211_contracts_port::{Completed, InFlight, SingleOutcome, TxEstimate};

pub fn finish<C, T, M>(
    operation: InFlight<C, T, M>,
    outcome: SingleOutcome<(), ()>,
) -> Completed<C, T, M, SingleOutcome<(), ()>> {
    #[cfg(ui_reject)]
    let terminal = Completed { operation, outcome, cost: TxEstimate::NONE }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let terminal = operation.complete(outcome, TxEstimate::NONE);
    terminal
}
