#![no_std]
use oer_radio_port::{Child, Line, Mint, Permits, Root};

pub enum Kind {}

pub fn line<'id>(
    mint: Mint<'id, (Permits<Kind, 1, 1>, Permits<Kind, 1, 1>)>,
) -> Line<'id, Kind, 1, Child<Root, 0>> {
    let (first, second) = mint.into_pools();
    #[cfg(ui_reject)]
    return second.line; // ui-error: E0308
    #[cfg(not(ui_reject))]
    first.line
}
