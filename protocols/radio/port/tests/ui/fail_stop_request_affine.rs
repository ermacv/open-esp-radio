#![no_std]
use oer_radio_port::{Ended, FailStopRequested, PortBackend};

pub fn ended<'id, B: PortBackend>(proof: FailStopRequested<'id, B>) -> Ended<'id, B, ()> {
    #[cfg(ui_reject)]
    let _second = proof.clone(); // ui-error: E0599
    Ended::fail_stop(proof, ())
}
