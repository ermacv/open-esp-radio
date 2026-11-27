#![no_std]
use oer_protocols_radio_port::{LocalId, Root, Slot};

pub trait Erased {}
impl<T> Erased for T {}

pub fn erase<'p>(slot: Slot<'p, (), Root<LocalId<'static>>>) -> impl Erased + 'static {
    #[cfg(ui_reject)]
    return slot; // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    drop(slot)
}
