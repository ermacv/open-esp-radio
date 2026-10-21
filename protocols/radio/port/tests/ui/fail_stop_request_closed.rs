#![no_std]
use oer_radio_port::{Ended, FailStopRequested, PortBackend};

pub fn ended<'id, Id: oer_radio_port::SlotIdentity, B: PortBackend<Id>>(request: FailStopRequested<Id, B>) -> Ended<Id, B, ()> {
    #[cfg(ui_reject)]
    let request = FailStopRequested { identity: core::marker::PhantomData }; // ui-error: E0451
    Ended::fail_stop(request, ())
}
