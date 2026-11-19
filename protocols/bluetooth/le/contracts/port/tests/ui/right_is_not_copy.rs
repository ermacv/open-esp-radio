#![no_std]
use oer_protocols_bluetooth_le_contracts_port::{Running, StopFrom};
pub fn twice<'id>(running: Running<'id>) -> StopFrom<'id> {
    let first = StopFrom::Running(running);
    #[cfg(ui_reject)]
    let _second = StopFrom::Running(running); // ui-error: E0382
    first
}
