#![no_std]
use oer_radio_port::{SlotClaim, SlotIdentity};

pub fn claim<Id: SlotIdentity>(original: SlotClaim<Id>) -> SlotClaim<Id> {
    #[cfg(ui_reject)]
    return SlotClaim { identity: core::marker::PhantomData }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    original
}
