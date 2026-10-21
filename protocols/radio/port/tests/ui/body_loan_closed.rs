#![no_std]
use oer_radio_port::{BodyLoan, SlotIdentity, SlotOwner};

pub fn loan<'p, Id: SlotIdentity, S: SlotOwner<Id>>(original: BodyLoan<'p, Id, S>) -> BodyLoan<'p, Id, S> {
    #[cfg(ui_reject)]
    return BodyLoan::new(); // ui-error: E0624
    #[cfg(not(ui_reject))]
    original
}
