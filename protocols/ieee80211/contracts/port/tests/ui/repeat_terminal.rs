#![no_std]
use oer_protocols_ieee80211_contracts_port::{InFlight, SingleOutcome, TxEstimate};
pub fn finish<C, T, M>(operation: InFlight<C, T, M>, outcome: SingleOutcome<(), ()>) {
    let terminal = operation.complete(outcome, TxEstimate::NONE);
    #[cfg(ui_reject)]
    let _again = operation.complete(outcome, TxEstimate::NONE); // ui-error: E0382
    drop(terminal);
}
