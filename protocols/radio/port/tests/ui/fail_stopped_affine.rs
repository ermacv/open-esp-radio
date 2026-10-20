#![no_std]
use oer_radio_port::{Ended, FailStopped, PortBackend};

pub fn ended<'id, B: PortBackend>(proof: FailStopped<'id, B>) -> Ended<'id, B, ()> {
    #[cfg(ui_reject)]
    let _second = proof.clone(); // ui-error: E0599
    Ended::fail_stopped(proof, ())
}
