#![no_std]
use oer_ieee80211_core_roles_sta::machine::{StaRequest, StaTokens};

pub fn request_for_station<'id, A: StaTokens, B: StaTokens>(
    local: StaRequest<'id, A::ConfigurationPath>,
    foreign: StaRequest<'id, B::ConfigurationPath>,
) -> StaRequest<'id, A::ConfigurationPath> {
    #[cfg(ui_reject)]
    return foreign; // ui-error: E0308
    #[cfg(not(ui_reject))]
    local
}
