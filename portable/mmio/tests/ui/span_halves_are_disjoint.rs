#![no_std]
use oer_mmio::Span;

#[cfg(not(ui_reject))]
const MID: usize = 0x1080;
// A split point outside the span does not compile.
#[cfg(ui_reject)]
const MID: usize = 0x1200;

pub const HALVES: (Span<0x1000, MID>, Span<MID, 0x1100>) =
    // SAFETY: a constant no image uses; nothing accesses the registers.
    unsafe { Span::<0x1000, 0x1100>::assume_owned() }.split_at::<MID>(); // ui-error@outside: E0080: evaluation panicked: a span splits at a word-aligned address inside it
