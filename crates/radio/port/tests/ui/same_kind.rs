#![no_std]
use oer_radio_port::{Child, Mint, Root, Slot, Slots};

pub enum Kind {}

pub fn select<'id>(
    mint: Mint<'id, (Slots<Kind, 1>, Slots<Kind, 1>)>,
) -> Slot<'id, Kind, Child<Root, 0>> {
    let ([first], [second]) = mint.into_pools();
    #[cfg(ui_reject)]
    return second; // ui-error: E0308
    #[cfg(not(ui_reject))]
    first
}
