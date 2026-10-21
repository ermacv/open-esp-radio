#![no_std]
use oer_radio_port::{SlotCompletion, SlotIdentity};
pub fn completion<Id: SlotIdentity>(completion: SlotCompletion<'_, Id>) {
    #[cfg(ui_reject)]
    let _ = completion.clone(); // ui-error: E0599
    completion.unregistered();
}
