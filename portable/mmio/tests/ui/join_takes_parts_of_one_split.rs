#![no_std]
use oer_mmio::{Block, Fixed, Span, Words};

pub struct Low;
impl Words for Low {
    const OWNED: &'static [usize] = &[0];
    const OBSERVED: &'static [usize] = &[];
}

pub struct High;
impl Words for High {
    const OWNED: &'static [usize] = &[4];
    const OBSERVED: &'static [usize] = &[];
}

type First = Block<Fixed<0x1000>, 8>;

pub fn rejoin(first: Span<0x1000, 0x1008>, second: Span<0x1008, 0x1010>) {
    let (low, first_high) = first.into_block::<8>().split::<Low, High>();
    let (second_low, second_high) = second.into_block::<8>().split::<Low, High>();
    // The parts of the one split of the block at 0x1000 join.
    #[cfg(not(ui_reject))]
    let (joined, rest) = (First::join(low, first_high), second_high);
    // A part of another block's split does not join with them.
    #[cfg(ui_reject)]
    let (joined, rest) = (First::join(low, second_high), first_high); // ui-error: E0308
    drop((joined, rest, second_low));
}
