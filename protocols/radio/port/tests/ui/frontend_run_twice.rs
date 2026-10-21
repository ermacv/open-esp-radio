#![no_std]
use oer_radio_port::{PortBackend, PortBody, SlotFrontend, SlotIdentity, run_port};
pub fn run<'s, Id: SlotIdentity, B: PortBackend<Id>>(frontend: SlotFrontend<'s, Id, B>, first: impl PortBody<Id, B>, second: impl PortBody<Id, B>) {
    let running = run_port(frontend, first);
    #[cfg(ui_reject)]
    let _ = run_port(frontend, second); // ui-error: E0382
    drop(running);
}
