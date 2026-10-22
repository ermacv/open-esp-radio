#![no_std]
#![allow(unused_variables)]
use oer_bluetooth_le_port::{LePhase, Request};
use oer_radio_port::{Root, SlotIdentity};
pub fn submit<'p, A: SlotIdentity, B: SlotIdentity, X, Own, Foreign>(
    own: &Own, foreign: &Foreign, request: Request<'p, Root<A>, X, Own::Caps>,
) where Own: LePhase<'p, Root<A>, X>, Foreign: LePhase<'p, Root<B>, X, Caps = Own::Caps> {
    #[cfg(ui_reject)]
    drop(foreign.submit(request)); // ui-error: E0308
    #[cfg(not(ui_reject))]
    drop(own.submit(request));
}
