#![no_std]
use oer_mmio::Span;

#[cfg(not(ui_reject))]
pub fn take() -> Span<0x1000, 0x1100> {
    // SAFETY: a test case no image runs.
    unsafe { Span::assume_owned() }
}

// Taking registers is the caller's unsafe promise.
#[cfg(ui_reject)]
pub fn take() -> Span<0x1000, 0x1100> {
    Span::assume_owned() // ui-error: E0133
}
