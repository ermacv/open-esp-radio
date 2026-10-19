#![no_std]
use oer_ieee80211_lower_mac::Ieee80211Capabilities;
use oer_ieee80211_sta::data::{RetiredData, StaDataTokens};

pub fn terminal<'id, T: StaDataTokens, K: Ieee80211Capabilities>(mut ended: RetiredData<'id, T, K>) {
    #[cfg(ui_reject)]
    drop(ended.poll_action()); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _ = ended.counters();
}
