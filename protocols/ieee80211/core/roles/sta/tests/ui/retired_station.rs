#![no_std]
use oer_ieee80211_contracts_port::Ieee80211Capabilities;
use oer_ieee80211_core_roles_sta::machine::{RetiredStation, StaTokens, StaRequest};
use oer_time::Instant;

pub fn terminal<'id, T: StaTokens, K: Ieee80211Capabilities, C: oer_ieee80211_core_roles_sta::machine::StaCallbacks>(mut ended: RetiredStation<'id, T, K, C>, request: StaRequest<'id, T::ConfigurationPath>) {
    #[cfg(ui_reject)]
    drop(ended.command(Instant::EPOCH, request)); // ui-error: E0599
    #[cfg(not(ui_reject))]
    drop(ended.poll_output());
}
