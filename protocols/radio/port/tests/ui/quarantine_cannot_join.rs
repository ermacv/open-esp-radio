#![no_std]
use oer_radio_port::{SlotIdentity, SlotQuarantined};
pub fn quarantine<Id: SlotIdentity>(outcome: SlotQuarantined<Id>) -> SlotQuarantined<Id> {
    #[cfg(ui_reject)]
    let _ = outcome.into_joined(); // ui-error: E0599
    outcome
}
