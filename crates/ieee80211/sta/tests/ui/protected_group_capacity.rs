#![no_std]
include!("../resources.rs");

#[cfg(ui_reject)]
const GROUPS: usize = 0;
#[cfg(not(ui_reject))]
const GROUPS: usize = 1;

pub fn build<'id>(
    config: StaConfig<Caps>,
    password: oer_ieee80211_rsn::sae::SaePassword,
    pmk: oer_ieee80211_rsn::Pmk,
    resources: StationResources<'id, Tokens<GROUPS>, 1, 1, GROUPS>,
) -> Station<'id, Tokens<GROUPS>, Caps> {
    Station::new_personal(config, password, pmk, resources) // ui-error: E0080
}

pub fn open_without_group_slots<'id>(
    config: StaConfig<Caps>,
    resources: StationResources<'id, Tokens<0>, 1, 1, 0>,
) -> Station<'id, Tokens<0>, Caps> {
    Station::new_open(config, resources)
}
