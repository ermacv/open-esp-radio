#![no_std]
use oer_radio_port::{SlotIdentity, SlotOwner, SlotRegistration};
pub fn guard<Id: SlotIdentity, S: SlotOwner<Id>>(registration: SlotRegistration<'_, Id, S>) {
    #[cfg(ui_reject)]
    let _armed = registration.armed; // ui-error: E0616
    #[cfg(not(ui_reject))]
    let _ = registration.is_quarantined();
}
