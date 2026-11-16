#![no_std]
use oer_protocols_ieee80211_contracts_port::{Command, Ieee80211Capabilities};
// A command carries the rights it needs: there is one of it.
pub fn twice<'id, K: Ieee80211Capabilities>(command: Command<'id, K>) -> Command<'id, K> {
    #[cfg(ui_reject)]
    let _another = command.clone(); // ui-error: E0599
    command
}
