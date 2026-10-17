#![no_std]
use oer_ieee80211_lower_mac::{ConfigurePermission, Station};
use oer_radio_port::{Child, Root};
pub fn use_first<'id>(
    permission: ConfigurePermission<'id, Station, Child<Root, 0>>,
) -> ConfigurePermission<'id, Station, Child<Root, 0>> {
    permission
}
pub fn configure<'id>(
    own: ConfigurePermission<'id, Station, Child<Root, 0>>,
    foreign: ConfigurePermission<'id, Station, Child<Root, 1>>,
) {
    #[cfg(ui_reject)]
    let _returned = use_first(foreign); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _returned = use_first(own);
}
