#![no_std]
use oer_mmio::{Block, Host, Registers};

pub struct Overflow;
impl Registers for Overflow {
    #[cfg(not(ui_reject))]
    const WORDS: &'static [usize] = &[usize::MAX - 7];
    // A last word ending past the address space must not wrap to a small
    // extent that a short host slice satisfies.
    #[cfg(ui_reject)]
    const WORDS: &'static [usize] = &[usize::MAX - 3];
}

// Placing the block in a word too short for it is refused, not accepted.
pub const REFUSED: bool = {
    let mut memory = [0_u32; 1];
    Block::<Host<'_>, Overflow>::host(&mut memory).is_err()
};
// ui-error@outside: E0080: evaluation panicked: a block's words lie inside the address space
