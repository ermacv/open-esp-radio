#![no_std]
use oer_ieee80211_port::{KeyTypes, LowerMacPhase, StationPhase, Station};
use oer_ieee80211_model::{ModelKeys, ModelPhase};

pub fn installation_belongs_to_port<'p, 'a: 'p, 'b: 'p>(
    target: &mut ModelPhase<'p, 'a, oer_ieee80211_model::ModelCaps, Composition>,
    permission: <ModelKeys<'a, Composition, Station> as KeyTypes<'a>>::GroupPermission,
    local: <ModelKeys<'a, Composition, Station> as KeyTypes<'a>>::GroupKey,
    foreign: <ModelKeys<'b, Composition, Station> as KeyTypes<'b>>::GroupKey,
) {
    #[cfg(ui_reject)]
    target.station().select_group_key(permission, foreign); // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    target.station().select_group_key(permission, local);
}

pub enum UiSlot {}
type Composition = oer_ieee80211_model::ModelBackend<oer_radio_port::StaticId<UiSlot>>;
