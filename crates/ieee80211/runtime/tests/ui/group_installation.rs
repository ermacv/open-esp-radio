#![no_std]
use oer_ieee80211_lower_mac::{KeyTypes, LowerMacPhase, StationPhase, Station};
use oer_ieee80211_runtime::lower_mac_owner::model::{ModelKeys, ModelPhase};

pub fn installation_belongs_to_port<'p, 'a: 'p, 'b: 'p>(
    target: &mut ModelPhase<'p, 'a>,
    permission: <ModelKeys<'a, Station> as KeyTypes<'a>>::GroupPermission,
    local: <ModelKeys<'a, Station> as KeyTypes<'a>>::GroupKey,
    foreign: <ModelKeys<'b, Station> as KeyTypes<'b>>::GroupKey,
) {
    #[cfg(ui_reject)]
    target.station().select_group_key(permission, foreign); // ui-error: null lifetime may not live long enough // ui-error: null lifetime may not live long enough
    #[cfg(not(ui_reject))]
    target.station().select_group_key(permission, local);
}
