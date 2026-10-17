#![no_std]
use oer_ieee80211_lower_mac::Ieee80211Capabilities;
use oer_ieee80211_sta::machine::{RetiredStation, StationRestart, StaTokens};

pub fn retired<'id, T: StaTokens, K: Ieee80211Capabilities>(station: RetiredStation<'id, T, K>) {
    #[cfg(ui_reject)]
    drop(station.deferred_eapol); // ui-error: E0616
    #[cfg(not(ui_reject))]
    drop(station.into_outputs());
}

pub fn restarted<'id, T: StaTokens, K: Ieee80211Capabilities>(station: StationRestart<'id, T, K>) {
    #[cfg(ui_reject)]
    drop(station.notice); // ui-error: E0616
    #[cfg(not(ui_reject))]
    drop(station.into_parts());
}
