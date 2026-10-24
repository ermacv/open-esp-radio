#![no_std]
use oer_mmio::{Block, Fixed, Registers};

pub struct Control;
impl Registers for Control {
    const WORDS: &'static [usize] = &[0, 4];
}

#[cfg(not(ui_reject))]
pub fn take() -> Block<Fixed<0x1000>, Control> {
    // SAFETY: a test case no image runs.
    unsafe { oer_mmio::assume_owned() }
}

// Taking registers is the caller's unsafe promise.
#[cfg(ui_reject)]
pub fn take() -> Block<Fixed<0x1000>, Control> {
    oer_mmio::assume_owned() // ui-error: E0133
}
