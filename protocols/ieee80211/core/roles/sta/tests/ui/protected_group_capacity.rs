#![no_std]
include!("../resources.rs");
use oer_protocols_ieee80211_core_roles_sta::machine::GroupCapacity;

#[cfg(ui_reject)]
const GROUPS: usize = 0;
#[cfg(not(ui_reject))]
const GROUPS: usize = 1;

pub const CAPACITY: GroupCapacity<GROUPS> = GroupCapacity::new(); // ui-error@outside: E0080: evaluation panicked: a station needs a group slot for any protected profile

pub fn build<'id>(
    config: StaConfig<Caps, Callbacks>,
    resources: StationResources<'id, Types<'id>>,
) -> (Station<'id, Types<'id>, Caps, Callbacks>, oer_protocols_ieee80211_core_roles_sta::machine::StaCommandCredits<'id>) {
    Station::new(config, resources, CAPACITY)
}
