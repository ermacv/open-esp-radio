#![no_std]
use oer_protocols_ieee80211_contracts_port::{KeyTypes, PortCommand, Station};
use oer_protocols_ieee80211_testing_model::{ModelCaps, ModelKeys, ModelTypes};

pub fn owner_moves_once<'id>(
    permission: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::GroupPermission,
    selected: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::GroupKey,
) -> PortCommand<'id, ModelTypes<Composition>, ModelCaps> {
    let select = PortCommand::SelectGroup { permission, installed: selected };
    #[cfg(ui_reject)]
    let _removed_while_selected = selected; // ui-error: E0382
    select
}

pub enum UiSlot {}
type Composition = oer_protocols_ieee80211_testing_model::ModelBackend<oer_protocols_radio_port::StaticId<UiSlot>>;
