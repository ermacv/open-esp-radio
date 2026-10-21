#![no_std]
use oer_radio_port::{SlotIdentity, SlotOwner, SlotRegistration};
pub fn run<Id: SlotIdentity, S: SlotOwner<Id>>(mut registration: SlotRegistration<'_, Id, S>, cx: &mut core::task::Context<'_>) {
    let running = registration.run();
    #[cfg(ui_reject)]
    let _ = registration.poll_request(cx); // ui-error: E0382
    drop(running);
}
