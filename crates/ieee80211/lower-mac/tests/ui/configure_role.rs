#![no_std]
use oer_ieee80211_lower_mac::{AccessPoint, ConfigurePermission, Station};
pub fn station<'id>(
    permission: ConfigurePermission<'id, Station>,
) -> ConfigurePermission<'id, Station> {
    permission
}
pub fn interface<'id>(
    own: ConfigurePermission<'id, Station>,
    foreign: ConfigurePermission<'id, AccessPoint>,
) {
    #[cfg(ui_reject)]
    let _returned = station(foreign); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _returned = station(own);
}
