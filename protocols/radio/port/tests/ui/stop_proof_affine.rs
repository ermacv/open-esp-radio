#![no_std]
use oer_radio_port::{SlotIdentity, SlotOwner, SlotStopped};
pub fn proof<Id: SlotIdentity, S: SlotOwner<Id>>(proof: SlotStopped<Id, S>) -> SlotStopped<Id, S> {
    #[cfg(ui_reject)]
    let _ = proof.clone(); // ui-error: E0599
    proof
}
