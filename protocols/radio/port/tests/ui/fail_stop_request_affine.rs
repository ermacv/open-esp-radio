#![no_std]
use oer_radio_port::{Ended, FailStopRequested, PortBackend};

pub fn ended<'id, Id: oer_radio_port::SlotIdentity, B: PortBackend<Id>>(proof: FailStopRequested<Id, B>) -> Ended<Id, B, ()> {
    #[cfg(ui_reject)]
    let _second = proof.clone(); // ui-error: E0599
    Ended::fail_stop(proof, ())
}
