#![no_std]
use oer_radio_port::{SlotCompletion, SlotIdentity};
pub fn completion<'s, Id: SlotIdentity, Other: SlotIdentity>(own: SlotCompletion<'s, Id>, foreign: SlotCompletion<'s, Other>) -> SlotCompletion<'s, Id> {
    #[cfg(ui_reject)]
    return foreign; // ui-error: E0308
    #[cfg(not(ui_reject))]
    own
}
