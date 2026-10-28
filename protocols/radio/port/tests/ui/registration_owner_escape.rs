#![no_std]
use oer_radio_port::{SlotIdentity, SlotOwner, SlotRegistration};
pub fn owner<Id: SlotIdentity, S: SlotOwner<Id>>(registration: SlotRegistration<'_, Id, S>) {
    #[cfg(ui_reject)]
    let _owner = registration.owner; // ui-error: E0616
    #[cfg(not(ui_reject))]
    let _ = registration.failure().is_some();
}
