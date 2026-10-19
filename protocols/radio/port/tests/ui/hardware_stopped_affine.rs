#![no_std]
use oer_radio_port::{Abandon, HardwareStopped};
pub fn confirmed(proof: HardwareStopped) -> Abandon {
    #[cfg(ui_reject)]
    let _second = proof.clone(); // ui-error: E0599
    Abandon::Stopped(proof)
}
