#![no_std]
use oer_ieee80211_contracts_port::{ConfigurePermission, Station};
use oer_radio_port::{Child, Root};
pub fn use_first<'id>(
    permission: ConfigurePermission<'id, Station, Child<Root<oer_radio_port::LocalId<'id>>, 0>>,
) -> ConfigurePermission<'id, Station, Child<Root<oer_radio_port::LocalId<'id>>, 0>> {
    permission
}
pub fn configure<'id>(
    own: ConfigurePermission<'id, Station, Child<Root<oer_radio_port::LocalId<'id>>, 0>>,
    foreign: ConfigurePermission<'id, Station, Child<Root<oer_radio_port::LocalId<'id>>, 1>>,
) {
    #[cfg(ui_reject)]
    let _returned = use_first(foreign); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _returned = use_first(own);
}
