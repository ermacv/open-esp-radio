#![no_std]
use oer_radio_port::{SlotIdentity, SlotOwner, SlotStopped};

pub fn stopped<Id: SlotIdentity, Other: SlotIdentity, S: SlotOwner<Id> + SlotOwner<Other>>(
    own: SlotStopped<Id, S>, foreign: SlotStopped<Other, S>,
) -> SlotStopped<Id, S> {
    #[cfg(ui_reject)]
    return foreign; // ui-error: E0308
    #[cfg(not(ui_reject))]
    own
}
