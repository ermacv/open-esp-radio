#![no_std]
use oer_ieee80211_arbiter::{StationPlaces, StationPlaceReturns};
use oer_ieee80211_lower_mac::Ieee80211Capabilities;
use oer_ieee80211_sta::machine::{RetiredStation, StaTokens};

pub fn restart<'id, T: StaTokens, K: Ieee80211Capabilities>(
    station: RetiredStation<'id, T, K>,
    complete: StationPlaces<'id, T::GatePath>,
    pending: StationPlaceReturns<'id, T::GatePath>,
) {
    #[cfg(ui_reject)]
    let restarted = station.restart(pending); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let restarted = station.restart(complete);
    drop(restarted.into_parts());
}
