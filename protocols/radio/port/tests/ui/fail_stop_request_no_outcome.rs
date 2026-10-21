#![no_std]
use oer_radio_port::{Ended, FailStopRequested, PortBackend};

pub fn finish<'id, B: PortBackend>(request: FailStopRequested<'id, B>) -> Ended<'id, B, ()> {
    #[cfg(ui_reject)]
    let _ = request.outcome(); // ui-error: E0599
    Ended::fail_stop(request, ())
}
