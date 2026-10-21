#![no_std]
use oer_radio_port::{PortSlot, SlotOwner, StaticId};

pub enum Declaration {}
pub fn end<S: SlotOwner<StaticId<Declaration>>>(slot: PortSlot<StaticId<Declaration>, S>) {
    #[cfg(ui_reject)]
    slot.end_model_world(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    drop(slot);
}
