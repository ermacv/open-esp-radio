#![no_std]
use oer_protocols_bluetooth_le_contracts_port::Stopped;
pub fn copy<'id>(stopped: Stopped<'id>) -> Stopped<'id> {
    #[cfg(ui_reject)]
    let _twice = stopped.clone(); // ui-error: E0599
    stopped
}
