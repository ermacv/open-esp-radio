#![no_std]
use oer_memory::{DmaIndexReturn, IndexedStableDmaLease, ReturningStableDmaBacking};
pub fn keep<B: IndexedStableDmaLease, R: DmaIndexReturn>(mut owner: ReturningStableDmaBacking<B, R>, replacement: B) {
    #[cfg(not(ui_reject))]
    { let _: &B = &owner; drop(replacement); }
    #[cfg(ui_reject)]
    { *owner = replacement; } // ui-error: E0594
}
