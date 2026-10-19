#![no_std]
use oer_radio_port::{HardwareStopped, Quarantine};
pub fn stop(call: Quarantine) -> HardwareStopped {
    let stopped = call.stopped();
    #[cfg(ui_reject)]
    let _second = call.stopped(); // ui-error: E0382
    stopped
}
