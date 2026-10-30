#![no_std]
// The diagnostics exist in test-support builds.
// ui-requires: not(test-support)
use oer_ieee80211_contracts_port::Ieee80211Capabilities;
use oer_ieee80211_core_roles_sta::machine::{RetiredStation, StaTokens};

pub fn product_bank<'id, T: StaTokens, K: Ieee80211Capabilities, C: oer_ieee80211_core_roles_sta::machine::StaCallbacks>(mut bank: RetiredStation<'id, T, K, C>) {
    #[cfg(ui_reject)]
    let _ = bank.frame_credits(); // ui-error: E0599
    #[cfg(ui_reject)]
    let _ = bank.key_owners(); // ui-error: E0599
    #[cfg(ui_reject)]
    let _ = bank.lost_tokens(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _ = bank.poll_output();
}
