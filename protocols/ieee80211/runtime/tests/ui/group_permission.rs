#![no_std]
use oer_ieee80211_lower_mac::{InterfacePhase, KeyTypes, LowerMacPhase, StationPhase, Station};
use oer_ieee80211_runtime::lower_mac_owner::model::{ModelKeys, ModelPhase};

pub fn one_mutation<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    permission: <ModelKeys<'id, Station> as KeyTypes<'id>>::GroupPermission,
    selected: <ModelKeys<'id, Station> as KeyTypes<'id>>::GroupKey,
    retained: <ModelKeys<'id, Station> as KeyTypes<'id>>::GroupKey,
) {
    phase.station().select_group_key(permission, selected);
    #[cfg(ui_reject)]
    phase.station().remove_group_key(permission, retained); // ui-error: E0382
    #[cfg(not(ui_reject))]
    let _retained_until_terminal = retained;
}
