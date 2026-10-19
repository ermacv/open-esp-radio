#![no_std]
use oer_ieee80211_sta::machine::StaRequest;
use oer_radio_port::PoolPath;

pub fn request_for_installation<'a, 'b, Path: PoolPath>(
    local: StaRequest<'a, Path>,
    foreign: StaRequest<'b, Path>,
) -> StaRequest<'a, Path> {
    #[cfg(ui_reject)]
    return foreign; // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    local
}
