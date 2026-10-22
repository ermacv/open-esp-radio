#![no_std]
#![allow(unused_variables)]
use oer_bluetooth_le_port::{Request, LeCapabilities};
use oer_radio_port::{PoolPath, PortContext};
pub fn context<'p, P: PoolPath, X, Y, C: LeCapabilities>(
    request: Request<'p, P, X, C>, foreign: PortContext<'p, P::Identity, Y>,
) -> Request<'p, P, X, C> {
    #[cfg(ui_reject)]
    return Request { context: foreign, ..request }; // ui-error: E0308
    #[cfg(not(ui_reject))]
    request
}
