#![no_std]

#[inline(never)]
#[unsafe(no_mangle)]
pub fn leaf(value: u32) -> u32 {
    core::hint::black_box(value).wrapping_mul(3)
}
