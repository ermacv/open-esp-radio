#![no_std]
use oer_ieee80211_core_arbiter::{StationPlaces, StationPlaceReturns};
use oer_ieee80211_contracts_port::Ieee80211Capabilities;
use oer_ieee80211_core_roles_sta::machine::{RetiredStation, StaTokens};

pub fn restart<'id, T: StaTokens, K: Ieee80211Capabilities, C: oer_ieee80211_core_roles_sta::machine::StaCallbacks>(
    station: RetiredStation<'id, T, K, C>,
    complete: StationPlaces<'id, T::GatePath>,
    pending: StationPlaceReturns<'id, T::GatePath>,
) {
    #[cfg(ui_reject)]
    let restarted = station.restart(pending); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let restarted = station.restart(complete);
    drop(restarted.into_parts());
}
