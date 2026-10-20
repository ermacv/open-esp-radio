#![no_std]
use oer_ieee80211_port::InFlight;
use oer_ieee80211_runtime::lower_mac_owner::model::ModelMpdu;

pub fn reorder<'id, C, T>(
    first: &mut InFlight<C, T, ModelMpdu<'id>>,
    second: &mut InFlight<C, T, ModelMpdu<'id>>,
) {
    #[cfg(ui_reject)]
    core::mem::swap(
        first.execute().mpdu, // ui-error: E0616
        second.execute().mpdu, // ui-error: E0616
    );
    #[cfg(not(ui_reject))]
    core::mem::swap(first, second);
}
