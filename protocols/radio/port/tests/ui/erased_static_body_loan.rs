#![no_std]
use oer_radio_port::{Root, Slot, StaticId};

pub enum Declaration {}
pub trait Erased {}
impl<T> Erased for T {}

pub fn erase<'p>(slot: Slot<'p, (), Root<StaticId<Declaration>>>) -> impl Erased + 'static {
    #[cfg(ui_reject)]
    return slot; // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    drop(slot)
}
