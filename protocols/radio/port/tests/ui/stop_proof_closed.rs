#![no_std]
use oer_radio_port::{SlotIdentity, SlotOwner, SlotStopped};
pub fn proof<Id: SlotIdentity, S: SlotOwner<Id>>(original: SlotStopped<Id, S>) -> SlotStopped<Id, S> {
    #[cfg(ui_reject)]
    return SlotStopped { identity: core::marker::PhantomData, owner: core::marker::PhantomData }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    original
}
