#![no_std]
#![allow(unused_variables)]
use oer_bluetooth_le_port::{CommandCapacity, LePhase};
use oer_radio_port::{Child, Permit, PoolPath};
pub fn release<'p, P: PoolPath, X>(phase: &impl LePhase<'p, P, X>, capacity: Permit<'p, CommandCapacity, Child<P, 0>>) {
    phase.release(capacity);
    #[cfg(ui_reject)]
    phase.release(capacity); // ui-error: E0382
}
