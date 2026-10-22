#![no_std]
#![allow(unused_variables)]
use oer_bluetooth_le_port::{Idle, Session};
use oer_radio_port::PoolPath;
#[cfg(ui_reject)]
pub fn escape<'p, P: PoolPath>(session: Session<'p, P, Idle>) -> Session<'static, P, Idle> {
    session // ui-error: lifetime may not live long enough
}
#[cfg(not(ui_reject))]
pub fn escape<'p, P: PoolPath>(session: Session<'p, P, Idle>) -> Session<'p, P, Idle> { session }
