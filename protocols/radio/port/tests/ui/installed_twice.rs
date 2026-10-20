#![no_std]
use oer_radio_port::{Installed, PortBackend, PortBody, run_port};
pub fn install<B: PortBackend>(
    installed: Installed<B>,
    first: impl PortBody<B>,
    second: impl PortBody<B>,
) {
    let running = run_port(installed, first, oer_radio_port::LocalExecution);
    #[cfg(ui_reject)]
    let _ = run_port(installed, second, oer_radio_port::LocalExecution); // ui-error: E0382
    drop(running);
}
