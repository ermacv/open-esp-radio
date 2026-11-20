#![no_std]
use oer_protocols_bluetooth_le_contracts_port::{
    ConnEvent, ConnPlace, Connection, LePort, RoleRef, TxPlace,
};
pub fn close<'id, P: LePort<'id, 1, 0, 1, 2, 1, 2, 0>>(
    port: &mut P,
    role: Connection<'id>,
    events: [ConnEvent<'id>; 2],
    tx: [TxPlace<'id>; 1],
) -> ConnPlace<'id> {
    let place = port.close_connection(role, events, tx);
    #[cfg(ui_reject)]
    port.withdraw(RoleRef::Connection(&role)); // ui-error: E0382
    place
}
