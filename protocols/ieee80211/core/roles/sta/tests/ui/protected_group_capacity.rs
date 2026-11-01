#![no_std]
include!("../resources.rs");
use oer_ieee80211_core_roles_sta::machine::GroupCapacity;

#[cfg(ui_reject)]
const GROUPS: usize = 0;
#[cfg(not(ui_reject))]
const GROUPS: usize = 1;

pub const CAPACITY: GroupCapacity<GROUPS> = GroupCapacity::new(); // ui-error@outside: E0080: evaluation panicked: a station needs a group slot for any protected profile

pub fn build<'id>(
    config: StaConfig<Caps, Callbacks>,
    resources: StationResources<'id, Tokens<'id, GROUPS>, 1, 1, GROUPS>,
) -> (Station<'id, Tokens<'id, GROUPS>, Caps, Callbacks>, oer_ieee80211_core_roles_sta::machine::StaCommandCredits<'id>) {
    Station::new(config, resources, CAPACITY)
}
