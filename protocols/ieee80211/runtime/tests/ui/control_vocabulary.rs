#![no_std]
use oer_ieee80211_runtime::station::Request;
#[cfg(not(ui_reject))]
use oer_ieee80211_runtime::station::contract::Command;
pub fn product_request() -> Request {
    #[cfg(ui_reject)]
    let kind = oer_ieee80211_sta::machine::StaCommand::Connect;
    #[cfg(not(ui_reject))]
    let kind = Command::Connect;
    Request::new(7, kind) // ui-error: E0308
}
