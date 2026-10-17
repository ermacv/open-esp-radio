#![no_std]
use oer_ieee80211_lower_mac::InFlight;
use oer_ieee80211_runtime::lower_mac_owner::model::ModelMpdu;

pub fn reorder<'id, C, T>(
    first: &mut InFlight<C, T, ModelMpdu<'id>>,
    second: &mut InFlight<C, T, ModelMpdu<'id>>,
) {
    #[cfg(ui_reject)]
    core::mem::swap(first.execute().mpdu, second.execute().mpdu); // ui-error: E0616 // ui-error: E0616
    #[cfg(not(ui_reject))]
    core::mem::swap(first, second);
}
