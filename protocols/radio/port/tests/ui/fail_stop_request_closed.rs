#![no_std]
use oer_radio_port::{Ended, FailStopRequested, PortBackend};

pub fn ended<'id, B: PortBackend>(request: FailStopRequested<'id, B>) -> Ended<'id, B, ()> {
    #[cfg(ui_reject)]
    let request = FailStopRequested {
        brand: core::marker::PhantomData, // ui-error: E0451
        backend: core::marker::PhantomData,
    };
    Ended::fail_stop(request, ())
}
