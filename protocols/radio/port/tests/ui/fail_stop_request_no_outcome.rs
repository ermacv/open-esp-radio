#![no_std]
use oer_radio_port::{Ended, FailStopRequested, PortBackend};

pub fn finish<'id, Id: oer_radio_port::SlotIdentity, B: PortBackend<Id>>(request: FailStopRequested<Id, B>) -> Ended<Id, B, ()> {
    #[cfg(ui_reject)]
    let _ = request.outcome(); // ui-error: E0599
    Ended::fail_stop(request, ())
}
