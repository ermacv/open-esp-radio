#![no_std]
use oer_ieee80211_port::PhaseTypes;
use oer_ieee80211_runtime::station_service::StationPort;

pub fn body_is_impossible<'id, B: StationPort<SlotId: 'id> + 'id>(
    body: <B::Types<'id> as PhaseTypes<'id>>::TxBody,
) {
    #[cfg(ui_reject)]
    let _body: [u8; 8] = body; // ui-error: E0308
    #[cfg(not(ui_reject))]
    match body {}
}
