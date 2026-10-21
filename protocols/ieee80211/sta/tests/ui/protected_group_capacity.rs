#![no_std]
include!("../resources.rs");
use oer_ieee80211_sta::machine::GroupCapacity;

#[cfg(ui_reject)]
const GROUPS: usize = 0;
#[cfg(not(ui_reject))]
const GROUPS: usize = 1;

pub const CAPACITY: GroupCapacity<GROUPS> = GroupCapacity::new(); // ui-error@outside: E0080: evaluation panicked: a protected station needs at least one group slot

pub fn build<'id>(
    config: StaConfig<Caps>,
    password: oer_ieee80211_rsn::sae::SaePassword,
    pmk: oer_ieee80211_rsn::Pmk,
    resources: StationResources<'id, Tokens<'id, GROUPS>, 1, 1, GROUPS>,
) -> (Station<'id, Tokens<'id, GROUPS>, Caps>, oer_ieee80211_sta::machine::StaCommandCredits<'id>) {
    Station::new_personal(config, password, pmk, resources, CAPACITY)
}

pub fn open_without_group_slots<'id>(
    config: StaConfig<Caps>,
    resources: StationResources<'id, Tokens<'id, 0>, 1, 1, 0>,
) -> (Station<'id, Tokens<'id, 0>, Caps>, oer_ieee80211_sta::machine::StaCommandCredits<'id>) {
    Station::new_open(config, resources)
}
