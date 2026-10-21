#![no_std]
use oer_radio_port::{Ended, FailStopRequested, PortBackend};

#[cfg(ui_reject)]
pub fn ended<'left, 'right, B: PortBackend>(proof: FailStopRequested<'left, B>) -> Ended<'right, B, ()>
where
    'left: 'right,
{
    Ended::fail_stop(proof, ()) // ui-error: lifetime may not live long enough
}
#[cfg(not(ui_reject))]
pub fn ended<'id, B: PortBackend>(proof: FailStopRequested<'id, B>) -> Ended<'id, B, ()> {
    Ended::fail_stop(proof, ())
}
