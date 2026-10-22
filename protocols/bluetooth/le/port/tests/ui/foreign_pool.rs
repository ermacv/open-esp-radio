#![no_std]
#![allow(unused_variables)]
use oer_bluetooth_le_port::{LePhase, Request};
use oer_radio_port::{Child, Root, SlotIdentity};
pub fn submit<'p, Id: SlotIdentity, X, A, B>(
    own: &A, foreign: &B,
    request: Request<'p, Child<Root<Id>, 0>, X, A::Caps>,
) where
    A: LePhase<'p, Child<Root<Id>, 0>, X>,
    B: LePhase<'p, Child<Root<Id>, 1>, X, Caps = A::Caps>,
{
    #[cfg(ui_reject)]
    drop(foreign.submit(request)); // ui-error: E0308
    #[cfg(not(ui_reject))]
    drop(own.submit(request));
}
