#![no_std]
use oer_radio_port::{SlotIdentity, SlotOwner, SlotRegistration};
pub fn run<Id: SlotIdentity, S: SlotOwner<Id>>(mut registration: SlotRegistration<'_, Id, S>) {
    let running = registration.drive();
    #[cfg(ui_reject)]
    let _ = registration.drive(); // ui-error: E0499
    drop(running);
}
