#![no_std]
use oer_protocols_ieee80211_contracts_port::PortTypes;
use oer_protocols_ieee80211_runtime::datapath::DatapathPort;

pub fn body_is_impossible<'id, D: DatapathPort<'id>>(
    body: <D::Types as PortTypes<'id>>::TxBody,
) {
    #[cfg(ui_reject)]
    let _body: [u8; 8] = body; // ui-error: E0308
    #[cfg(not(ui_reject))]
    match body {}
}
