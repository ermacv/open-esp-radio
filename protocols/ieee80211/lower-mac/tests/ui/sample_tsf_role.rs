#![no_std]
use oer_ieee80211_lower_mac::{AccessPoint, Station, TsfPermission};
pub fn station<'id>(permission: TsfPermission<'id, Station>) -> TsfPermission<'id, Station> {
    permission
}
pub fn interface<'id>(own: TsfPermission<'id, Station>, foreign: TsfPermission<'id, AccessPoint>) {
    #[cfg(ui_reject)]
    let _returned = station(foreign); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _returned = station(own);
}
