#![no_std]
use oer_protocols_ieee80211_contracts_port::{KeyTypes, PortCommand, Station};
use oer_protocols_ieee80211_testing_model::{ModelCaps, ModelKeys, ModelTypes};

pub fn one_mutation<'id>(
    permission: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::GroupPermission,
    selected: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::GroupKey,
    retained: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::GroupKey,
) -> PortCommand<'id, ModelTypes<Composition>, ModelCaps> {
    let select = PortCommand::SelectGroup { permission, installed: selected };
    #[cfg(ui_reject)]
    let _remove = PortCommand::<'id, ModelTypes<Composition>, ModelCaps>::RemoveGroup { permission, key: retained }; // ui-error: E0382
    #[cfg(not(ui_reject))]
    let _retained_until_terminal = retained;
    select
}

pub enum UiSlot {}
type Composition = oer_protocols_ieee80211_testing_model::ModelBackend<oer_protocols_radio_port::StaticId<UiSlot>>;
