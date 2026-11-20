#![no_std]
use oer_protocols_bluetooth_le_contracts_port::{
    ConnEvent, ConnPlace, Connection, ConnectionEvent, LePort, Supported, TxPlace,
};
pub fn close<'id, P: LePort<'id, 1, 0, 1, 2, 1, 2, 0>>(
    port: &mut P,
    role: Connection<'id>,
    [first, second]: [ConnEvent<'id>; 2],
    tx: [TxPlace<'id>; 1],
    event: &Supported<P::Caps, ConnectionEvent>,
) -> ConnPlace<'id> {
    #[cfg(ui_reject)]
    let _ = port.publish_connection(first, event);
    #[cfg(not(ui_reject))]
    let _ = event;
    port.close_connection(role, [first, second], tx) // ui-error: E0382
}
