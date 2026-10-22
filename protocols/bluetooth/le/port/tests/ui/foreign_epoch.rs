#![no_std]
#![allow(unused_variables)]
use oer_bluetooth_le_port::{Idle, Session};
use oer_radio_port::PoolPath;
pub fn exchange<'a, 'b: 'a, P: PoolPath>(
    own: Session<'a, P, Idle>, foreign: Session<'b, P, Idle>,
) -> Session<'a, P, Idle> {
    #[cfg(ui_reject)]
    return foreign; // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    own
}
