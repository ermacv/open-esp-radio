#![no_std]
use oer_radio_port::{Child, Root, Slot, StaticId};

pub enum First {}
pub enum Second {}
pub enum Kind {}

pub fn select<'p>(
    own: Slot<'p, Kind, Child<Root<StaticId<First>>, 0>>,
    foreign: Slot<'p, Kind, Child<Root<StaticId<Second>>, 0>>,
) -> Slot<'p, Kind, Child<Root<StaticId<First>>, 0>> {
    #[cfg(ui_reject)]
    return foreign; // ui-error: E0308
    #[cfg(not(ui_reject))]
    own
}
