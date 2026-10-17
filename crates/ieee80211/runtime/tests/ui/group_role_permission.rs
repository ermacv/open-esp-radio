#![no_std]
use oer_ieee80211_lower_mac::{AccessPoint, KeyTypes, LowerMacPhase, StationPhase, Station};
use oer_ieee80211_runtime::lower_mac_owner::model::{ModelKeys, ModelPhase};

pub fn permission_belongs_to_interface<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    station: <ModelKeys<'id, Station> as KeyTypes<'id>>::GroupPermission,
    ap: <ModelKeys<'id, AccessPoint> as KeyTypes<'id>>::GroupPermission,
    selected: <ModelKeys<'id, Station> as KeyTypes<'id>>::GroupKey,
) {
    #[cfg(ui_reject)]
    phase.station().select_group_key(ap, selected); // ui-error: E0308
    #[cfg(not(ui_reject))]
    phase.station().select_group_key(station, selected);
}
