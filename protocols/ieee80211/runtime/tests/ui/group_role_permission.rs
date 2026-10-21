#![no_std]
use oer_ieee80211_port::{AccessPoint, KeyTypes, LowerMacPhase, StationPhase, Station};
use oer_ieee80211_model::{ModelKeys, ModelPhase};

pub fn permission_belongs_to_interface<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id, oer_ieee80211_model::ModelCaps, Composition>,
    station: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::GroupPermission,
    ap: <ModelKeys<'id, Composition, AccessPoint> as KeyTypes<'id>>::GroupPermission,
    selected: <ModelKeys<'id, Composition, Station> as KeyTypes<'id>>::GroupKey,
) {
    #[cfg(ui_reject)]
    phase.station().select_group_key(ap, selected); // ui-error: E0308
    #[cfg(not(ui_reject))]
    phase.station().select_group_key(station, selected);
}

pub enum UiSlot {}
type Composition = oer_ieee80211_model::ModelBackend<oer_radio_port::StaticId<UiSlot>>;
