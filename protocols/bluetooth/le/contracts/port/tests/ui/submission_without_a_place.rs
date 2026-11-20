#![no_std]
use oer_protocols_bluetooth_le_contracts_port::{LePort, TxPlace};
pub fn submit<'id, P: LePort<'id, 1, 0, 1, 2, 1, 2, 0>>(port: &mut P, place: TxPlace<'id>) {
    port.submit(place);
    #[cfg(ui_reject)]
    port.submit(place); // ui-error: E0382
}
