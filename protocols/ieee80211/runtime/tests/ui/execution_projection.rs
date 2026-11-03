#![no_std]
use oer_protocols_ieee80211_contracts_port::InFlight;
#[cfg(ui_reject)]
use oer_protocols_ieee80211_contracts_port::TxMpdu;
use oer_protocols_ieee80211_testing_model::ModelMpdu;

pub fn lend<'id, C, T>(
    single: &mut InFlight<C, T, ModelMpdu<'id, Composition>>,
) {
    let mut single = single.execute();
    #[cfg(ui_reject)]
    {
        let _: &mut ModelMpdu<'id, Composition> = &mut single; // ui-error: E0308
        single.into_parts(); // ui-error: E0599
    }
    #[cfg(not(ui_reject))]
    core::hint::black_box(&mut single);
}

pub enum UiSlot {}
type Composition = oer_protocols_ieee80211_testing_model::ModelBackend<oer_protocols_radio_port::StaticId<UiSlot>>;
