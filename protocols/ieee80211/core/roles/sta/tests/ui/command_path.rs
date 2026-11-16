#![no_std]
use oer_protocols_ieee80211_core_roles_sta::machine::{StaRequest, StaTypes};

pub fn request_for_station<'id, A: StaTypes, B: StaTypes>(
    local: StaRequest<'id, A::ConfigurationPath>,
    foreign: StaRequest<'id, B::ConfigurationPath>,
) -> StaRequest<'id, A::ConfigurationPath> {
    #[cfg(ui_reject)]
    return foreign; // ui-error: E0308
    #[cfg(not(ui_reject))]
    local
}
