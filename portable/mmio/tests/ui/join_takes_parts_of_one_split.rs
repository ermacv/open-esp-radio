#![no_std]
use oer_mmio::{Block, Fixed, Registers, Words};

pub struct Pair;
impl Registers for Pair {
    type Sharing = oer_mmio::sharing::Owned;
    const WORDS: &'static [usize] = &[0, 4];
}

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

type First = Block<Fixed<0x1000>, Pair>;
type Second = Block<Fixed<0x1008>, Pair>;

pub fn rejoin(first: First, second: Second) {
    let (low, first_high) = first.split::<Low, High>();
    let (second_low, second_high) = second.split::<Low, High>();
    // The parts of the one split of the block at 0x1000 join.
    #[cfg(not(ui_reject))]
    let (joined, rest) = (First::join(low, first_high), second_high);
    // A part of another block's split does not join with them.
    #[cfg(ui_reject)]
    let (joined, rest) = (First::join(low, second_high), first_high); // ui-error: E0308
    drop((joined, rest, second_low));
}
