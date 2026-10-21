#![no_std]
use oer_radio_port::{Ended, FailStopRequested, PortBackend, SlotIdentity};
#[cfg(ui_reject)]
pub fn ended<First: SlotIdentity, Second: SlotIdentity, B: PortBackend<First> + PortBackend<Second>>(proof: FailStopRequested<First, B>) -> Ended<Second, B, ()> {
    Ended::fail_stop(proof, ()) // ui-error: E0308
}
#[cfg(not(ui_reject))]
pub fn ended<Id: SlotIdentity, B: PortBackend<Id>>(proof: FailStopRequested<Id, B>) -> Ended<Id, B, ()> {
    Ended::fail_stop(proof, ())
}
