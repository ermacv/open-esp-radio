#![no_std]
use oer_radio_port::{Ended, FailStopped, PortBackend};

#[cfg(ui_reject)]
pub fn ended<'left, 'right, B: PortBackend>(proof: FailStopped<'left, B>) -> Ended<'right, B, ()>
where
    'left: 'right,
{
    Ended::fail_stopped(proof, ()) // ui-error: null lifetime may not live long enough
}
#[cfg(not(ui_reject))]
pub fn ended<'id, B: PortBackend>(proof: FailStopped<'id, B>) -> Ended<'id, B, ()> {
    Ended::fail_stopped(proof, ())
}
