#![no_std]
use oer_portable_mmio::{Block, Host, Registers};

pub struct Overlap;
impl Registers for Overlap {
    type Sharing = oer_portable_mmio::sharing::Owned;
    const WORDS: &'static [usize] = &[0];
    #[cfg(not(ui_reject))]
    const BYTES: &'static [usize] = &[4, 5];
    // A byte register inside a word the block lists would be reached both
    // as itself and through its word.
    #[cfg(ui_reject)]
    const BYTES: &'static [usize] = &[1, 4];
}

pub const PLACED: bool = {
    let mut memory = [0_u32; 2];
    Block::<Host<'_>, Overlap>::host(&mut memory).is_ok()
};
// ui-error@outside: E0080: evaluation panicked: a block's bytes ascend and lie in no word it lists
