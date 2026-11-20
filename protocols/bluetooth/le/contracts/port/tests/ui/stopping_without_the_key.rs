#![no_std]
use oer_protocols_bluetooth_le_contracts_port::{PortKey, StopFrom, Stopping};
pub fn stop<'id>(key: &PortKey<'id, 2, 1>, from: StopFrom<'id>) -> Stopping<'id> {
    let _ = key;
    #[cfg(ui_reject)]
    let stopping = Stopping { brand: core::marker::PhantomData }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let stopping = key.stop(from);
    stopping
}
