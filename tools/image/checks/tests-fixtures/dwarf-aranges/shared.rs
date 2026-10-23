#![no_std]

#[inline(never)]
#[unsafe(link_section = ".ram")]
pub fn callee(value: u32) -> u32 {
    leaf::leaf(core::hint::black_box(value).wrapping_add(7))
}
