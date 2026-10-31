#![no_std]
use oer_ieee80211_contracts_port::InFlight;
use oer_ieee80211_testing_model::ModelMpdu;

pub fn reorder<'id, C, T>(
    first: &mut InFlight<C, T, ModelMpdu<'id, Composition>>,
    second: &mut InFlight<C, T, ModelMpdu<'id, Composition>>,
) {
    #[cfg(ui_reject)]
    core::mem::swap(
        first.execute().mpdu, // ui-error: E0616
        second.execute().mpdu, // ui-error: E0616
    );
    #[cfg(not(ui_reject))]
    core::mem::swap(first, second);
}

pub enum UiSlot {}
type Composition = oer_ieee80211_testing_model::ModelBackend<oer_radio_port::StaticId<UiSlot>>;
