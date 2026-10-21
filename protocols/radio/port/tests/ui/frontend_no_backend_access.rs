#![no_std]
use oer_radio_port::{SlotFrontend, SlotIdentity, SlotOwner};
pub fn frontend<Id: SlotIdentity, S: SlotOwner<Id>>(mut frontend: SlotFrontend<'_, Id, S>) {
    #[cfg(ui_reject)]
    let _ = frontend.owner(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _ = frontend.ports();
}
