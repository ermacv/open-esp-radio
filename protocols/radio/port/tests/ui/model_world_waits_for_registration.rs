#![no_std]
use oer_radio_port::{LocalId, ModelSlotOwner, PortSlot};

pub fn end<'scope, S: ModelSlotOwner<LocalId<'scope>>>(mut slot: PortSlot<LocalId<'scope>, S>, owner: S) {
    let (frontend, registration) = slot.register(owner).ok().expect("fresh slot");
    #[cfg(ui_reject)]
    slot.end_model_world(); // ui-error: E0505
    drop(frontend);
    drop(registration);
    #[cfg(not(ui_reject))]
    slot.end_model_world();
}
