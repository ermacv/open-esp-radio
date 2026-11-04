#![no_std]
use oer_protocols_ieee80211_contracts_port::{ConfigurePermission, Station};
pub fn permissions<'id>(configure: ConfigurePermission<'id, Station>) {
    #[cfg(ui_reject)]
    let _another = configure.clone(); // ui-error: E0599
    let _owner = configure;
}
