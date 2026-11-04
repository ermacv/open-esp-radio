#![no_std]
use oer_protocols_ieee80211_contracts_port::{KeyTypes, PortCommand, Station};
use oer_protocols_ieee80211_testing_model::{ModelCaps, ModelKeys, ModelTypes};

pub fn installation_belongs_to_port<'a, 'b>(
    permission: <ModelKeys<'a, Composition, Station> as KeyTypes<'a>>::GroupPermission,
    local: <ModelKeys<'a, Composition, Station> as KeyTypes<'a>>::GroupKey,
    foreign: <ModelKeys<'b, Composition, Station> as KeyTypes<'b>>::GroupKey,
) -> PortCommand<'a, ModelTypes<Composition>, ModelCaps> {
    #[cfg(ui_reject)]
    return PortCommand::SelectGroup { permission, installed: foreign }; // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    {
        let _ = foreign;
        PortCommand::SelectGroup { permission, installed: local }
    }
}

pub enum UiSlot {}
type Composition = oer_protocols_ieee80211_testing_model::ModelBackend<oer_protocols_radio_port::StaticId<UiSlot>>;
