#![no_std]
use oer_ieee80211_lower_mac::{ConfigurePermission, Station, TsfPermission};
pub fn permissions<'id>(
    configure: ConfigurePermission<'id, Station>,
    tsf: TsfPermission<'id, Station>,
) {
    #[cfg(ui_reject)]
    let _another = configure.clone(); // ui-error: E0599
    #[cfg(ui_reject)]
    let _another = tsf.clone(); // ui-error: E0599
    let _owners = (configure, tsf);
}
