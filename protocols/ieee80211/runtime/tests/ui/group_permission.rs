#![no_std]
use oer_ieee80211_contracts_port::{InterfacePhase, KeyTypes, LowerMacPhase, StationPhase, Station};
use oer_ieee80211_testing_model::{ModelKeys, ModelPhase};

pub fn one_mutation<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id, oer_ieee80211_testing_model::ModelCaps, Composition>,
    permission: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::GroupPermission,
    selected: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::GroupKey,
    retained: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::GroupKey,
) {
    phase.station().select_group_key(permission, selected);
    #[cfg(ui_reject)]
    phase.station().remove_group_key(permission, retained); // ui-error: E0382
    #[cfg(not(ui_reject))]
    let _retained_until_terminal = retained;
}

pub enum UiSlot {}
type Composition = oer_ieee80211_testing_model::ModelBackend<oer_radio_port::StaticId<UiSlot>>;
