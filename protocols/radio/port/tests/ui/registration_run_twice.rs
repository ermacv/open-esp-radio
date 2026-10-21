#![no_std]
use oer_radio_port::{SlotIdentity, SlotOwner, SlotRegistration};
pub fn run<Id: SlotIdentity, S: SlotOwner<Id>>(registration: SlotRegistration<'_, Id, S>) {
    let running = registration.run();
    #[cfg(ui_reject)]
    let _ = registration.run(); // ui-error: E0382
    drop(running);
}
