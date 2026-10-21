#![no_std]
use oer_radio_port::{Ended, FailStopRequested, PortBackend, SlotIdentity};
pub fn end<Id: SlotIdentity, B: PortBackend<Id>>(request: FailStopRequested<Id, B>) {
    let first = Ended::fail_stop(request, ());
    #[cfg(ui_reject)]
    let _ = Ended::fail_stop(request, ()); // ui-error: E0382
    drop(first);
}
