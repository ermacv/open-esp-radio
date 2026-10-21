#![no_std]
use oer_radio_port::{Joined, PortSlot, SlotIdentity, SlotOwner};

pub fn reinstall<Id: SlotIdentity, Other: SlotIdentity, S: SlotOwner<Id>>(
    slot: &mut PortSlot<Id, S>, own: Joined<Id>, foreign: Joined<Other>, owner: S,
) {
    #[cfg(ui_reject)]
    let _ = slot.reinstall(foreign, owner); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _ = slot.reinstall(own, owner);
}
