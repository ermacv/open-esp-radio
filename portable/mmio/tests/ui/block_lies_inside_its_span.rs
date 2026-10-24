#![no_std]
use oer_mmio::{Block, Fixed, Span};

#[cfg(not(ui_reject))]
const SIZE: usize = 0x100;
// A block reaching past its span does not compile.
#[cfg(ui_reject)]
const SIZE: usize = 0x104;

pub const BLOCK: Block<Fixed<0x1000>, SIZE> =
    // SAFETY: a constant no image uses; nothing accesses the block.
    unsafe { Span::<0x1000, 0x1100>::assume_owned() }.into_block::<SIZE>(); // ui-error@outside: E0080: evaluation panicked: a block lies inside its span
