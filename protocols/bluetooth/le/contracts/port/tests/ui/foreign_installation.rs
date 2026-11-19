#![no_std]
use oer_protocols_bluetooth_le_contracts_port::{LePort, Starting, Stopped};
pub fn start<'a, 'b: 'a, P: LePort<'a, 1, 0, 1, 2, 1, 2, 0>>(
    port: &mut P,
    own: Stopped<'a>,
    foreign: Stopped<'b>,
) -> Starting<'a> {
    let _ = &foreign;
    #[cfg(ui_reject)]
    return port.start(foreign); // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    port.start(own)
}
