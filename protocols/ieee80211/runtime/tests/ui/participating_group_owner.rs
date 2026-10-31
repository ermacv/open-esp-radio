#![no_std]
use oer_ieee80211_contracts_port::{InterfacePhase, KeyTypes, LowerMacPhase, StationPhase, Station};
use oer_ieee80211_testing_model::{ModelKeys, ModelPhase};

pub fn owner_moves_once<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id, oer_ieee80211_testing_model::ModelCaps, Composition>,
    permission: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::GroupPermission,
    selected: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::GroupKey,
) {
    phase.station().select_group_key(permission, selected);
    #[cfg(ui_reject)]
    let _removed_while_selected = selected; // ui-error: E0382
}

pub enum UiSlot {}
type Composition = oer_ieee80211_testing_model::ModelBackend<oer_radio_port::StaticId<UiSlot>>;
