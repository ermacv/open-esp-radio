#![no_std]
use oer_radio_port::{Joined, SlotIdentity};

pub fn joined<Id: SlotIdentity>(original: Joined<Id>) -> Joined<Id> {
    #[cfg(ui_reject)]
    return Joined { identity: core::marker::PhantomData }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    original
}
