#![no_std]
use oer_radio_port::{Joined, PortSlot, SlotIdentity, SlotOwner, SlotQuarantined};

pub fn reinstall<Id: SlotIdentity, S: SlotOwner<Id>>(
    slot: &mut PortSlot<Id, S>, joined: Joined<Id>, quarantined: SlotQuarantined<Id>, owner: S,
) {
    #[cfg(ui_reject)]
    let _ = slot.reinstall(quarantined, |_| owner); // ui-error: E0308
    #[cfg(not(ui_reject))]
    let _ = slot.reinstall(joined, |_| owner);
}
