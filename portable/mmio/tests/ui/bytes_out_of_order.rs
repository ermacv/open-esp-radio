#![no_std]
use oer_portable_mmio::{Block, Host, Registers};

pub struct Unordered;
impl Registers for Unordered {
    type Sharing = oer_portable_mmio::sharing::Owned;
    const WORDS: &'static [usize] = &[0];
    #[cfg(not(ui_reject))]
    const BYTES: &'static [usize] = &[4, 5];
    // Byte registers out of order: the lists the accesses search are
    // strictly ascending.
    #[cfg(ui_reject)]
    const BYTES: &'static [usize] = &[5, 4];
}

pub const PLACED: bool = {
    let mut memory = [0_u32; 2];
    Block::<Host<'_>, Unordered>::host(&mut memory).is_ok()
};
// ui-error@outside: E0080: evaluation panicked: a block's bytes ascend and lie in no word it lists
