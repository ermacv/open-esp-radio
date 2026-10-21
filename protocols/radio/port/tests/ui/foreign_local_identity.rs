#![no_std]
use oer_radio_port::{Child, LocalId, Root, Slot};

pub enum Kind {}

pub fn select<'p, 'first, 'second>(
    own: Slot<'p, Kind, Child<Root<LocalId<'first>>, 0>>,
    foreign: Slot<'p, Kind, Child<Root<LocalId<'second>>, 0>>,
) -> Slot<'p, Kind, Child<Root<LocalId<'first>>, 0>> {
    #[cfg(ui_reject)]
    return foreign; // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    own
}
