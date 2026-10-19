#![no_std]
use oer_ieee80211_lower_mac::{InterfacePhase, KeyTypes, LowerMacPhase, StationPhase, Station};
use oer_ieee80211_runtime::lower_mac_owner::model::{ModelKeys, ModelPhase};

pub fn owner_moves_once<'p, 'id: 'p>(
    phase: &mut ModelPhase<'p, 'id>,
    permission: <ModelKeys<'id, Station> as KeyTypes<'id>>::GroupPermission,
    selected: <ModelKeys<'id, Station> as KeyTypes<'id>>::GroupKey,
) {
    phase.station().select_group_key(permission, selected);
    #[cfg(ui_reject)]
    let _removed_while_selected = selected; // ui-error: E0382
}
