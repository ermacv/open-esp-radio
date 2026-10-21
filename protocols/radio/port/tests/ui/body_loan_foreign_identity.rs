#![no_std]
use oer_radio_port::{BodyLoan, Root, Slot, SlotIdentity, SlotOwner};

pub fn reclaim<'p, Id: SlotIdentity, Other: SlotIdentity, S: SlotOwner<Id>>(
    loan: BodyLoan<'p, Id, S>, own: Slot<'p, (), Root<Id>>, foreign: Slot<'p, (), Root<Other>>,
) {
    #[cfg(ui_reject)]
    let _ = loan.reclaim_slot(foreign); // ui-error: E0271
    #[cfg(not(ui_reject))]
    drop(loan.reclaim_slot(own));
}
