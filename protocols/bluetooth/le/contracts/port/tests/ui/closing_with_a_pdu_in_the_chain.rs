#![no_std]
use oer_protocols_bluetooth_le_contracts_port::{ConnEvent, ConnPlace, Connection, LePort, TxPlace};
pub fn close<'id, P: LePort<'id, 1, 0, 1, 2, 1, 2, 0>>(
    port: &mut P,
    role: Connection<'id>,
    events: [ConnEvent<'id>; 2],
    [tx]: [TxPlace<'id>; 1],
) -> ConnPlace<'id> {
    #[cfg(ui_reject)]
    port.submit(tx);
    port.close_connection(role, events, [tx]) // ui-error: E0382
}
