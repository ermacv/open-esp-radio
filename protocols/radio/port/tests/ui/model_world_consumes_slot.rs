#![no_std]
use oer_radio_port::{LocalId, ModelSlotOwner, PortSlot};

pub fn end<'scope, S: ModelSlotOwner<LocalId<'scope>>>(slot: PortSlot<LocalId<'scope>, S>) {
    slot.end_model_world();
    #[cfg(ui_reject)]
    slot.end_model_world(); // ui-error: E0382
}
