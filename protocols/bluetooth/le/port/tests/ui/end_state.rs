#![no_std]
#![allow(unused_variables)]
use oer_bluetooth_le_port::{DtmCommand, Idle, LeCapabilities, Running, Session};
use oer_radio_port::PoolPath;
pub fn end<'p, P: PoolPath, C: LeCapabilities>(idle: Session<'p, P, Idle>, running: Session<'p, P, Running>) -> DtmCommand<'p, P, C> {
    #[cfg(ui_reject)]
    return DtmCommand::End(idle); // ui-error: E0308
    #[cfg(not(ui_reject))]
    DtmCommand::End(running)
}
