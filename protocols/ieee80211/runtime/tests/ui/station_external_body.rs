#![no_std]
use oer_ieee80211_lower_mac::PhaseTypes;
use oer_ieee80211_runtime::station_service::StationPort;

pub fn body_is_impossible<'id, B: StationPort<'id>>(
    body: <B::Types as PhaseTypes<'id>>::TxBody,
) {
    #[cfg(ui_reject)]
    let _body: [u8; 8] = body; // ui-error: E0308
    #[cfg(not(ui_reject))]
    match body {}
}
