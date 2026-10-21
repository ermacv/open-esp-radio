#![no_std]
use oer_radio_port::{Joined, PortSlot, SlotIdentity, SlotOwner};

pub fn reinstall<Id: SlotIdentity, S: SlotOwner<Id>>(
    slot: &mut PortSlot<Id, S>, joined: Joined<Id>, first: S, second: S,
) {
    let _ = slot.reinstall(joined, first);
    #[cfg(ui_reject)]
    let _ = slot.reinstall(joined, second); // ui-error: E0382
}
