#![no_std]
use oer_radio_port::{SlotIdentity, SlotOwner, SlotStopped};

pub fn stopped<Id: SlotIdentity, S: SlotOwner<Id>, Other: SlotOwner<Id>>(
    own: SlotStopped<Id, S>, foreign: SlotStopped<Id, Other>,
) -> SlotStopped<Id, S> {
    #[cfg(ui_reject)]
    return foreign; // ui-error: E0308
    #[cfg(not(ui_reject))]
    own
}
