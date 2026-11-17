#![no_std]
use oer_protocols_ieee80211_contracts_port::PairwiseCommand;
// A command carries the rights it needs: there is one of it.
pub fn twice<'id>(command: PairwiseCommand<'id>) -> PairwiseCommand<'id> {
    #[cfg(ui_reject)]
    let _another = command.clone(); // ui-error: E0599
    command
}
