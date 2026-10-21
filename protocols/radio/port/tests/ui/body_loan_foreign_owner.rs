#![no_std]
use oer_radio_port::{BodyLoan, SlotIdentity, SlotOwner};

pub fn loan<'p, Id: SlotIdentity, S: SlotOwner<Id>, Other: SlotOwner<Id>>(
    own: BodyLoan<'p, Id, S>, foreign: BodyLoan<'p, Id, Other>,
) -> BodyLoan<'p, Id, S> {
    #[cfg(ui_reject)]
    return foreign; // ui-error: E0308
    #[cfg(not(ui_reject))]
    own
}
