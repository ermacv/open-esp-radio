#![no_std]
use oer_radio_port::{LocalId, SlotIdentity, StaticId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Foreign;

#[cfg(ui_reject)]
impl SlotIdentity for Foreign {} // ui-error: E0277

#[cfg(not(ui_reject))]
pub fn accepted<'scope>() {
    fn identity<Id: SlotIdentity>() {}
    identity::<LocalId<'scope>>();
    identity::<StaticId<Foreign>>();
}
