#![no_std]
use oer_radio_port::{SlotCompletion, SlotIdentity};
pub fn unregister<Id: SlotIdentity>(completion: SlotCompletion<'_, Id>) {
    completion.unregistered();
    #[cfg(ui_reject)]
    completion.unregistered(); // ui-error: E0382
}
