#![no_std]
use oer_protocols_bluetooth_le_contracts_port::{LePort, Rest};
pub fn round<'id, P: LePort<'id, 1, 0, 1, 2, 1, 2, 0>>(port: &mut P) -> Rest {
    let round = port.round();
    #[cfg(ui_reject)]
    port.alarm(None); // ui-error: E0499
    round.close()
}
