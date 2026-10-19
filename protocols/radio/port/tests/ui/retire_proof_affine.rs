#![no_std]
use oer_radio_port::{DriverExit, RetireProof};
pub fn finish(proof: RetireProof) -> DriverExit<(), ()> {
    #[cfg(ui_reject)]
    let _ = proof.clone(); // ui-error: E0599
    DriverExit::Retired { state: (), proof }
}
