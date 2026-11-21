#![cfg_attr(
    target_arch = "riscv32",
    expect(
        clippy::cast_possible_truncation,
        reason = "R2 cleans the chip layers: family drivers and boot (tasks/312)"
    )
)]
#![cfg_attr(target_arch = "riscv32", no_main, no_std)]
#![deny(clippy::undocumented_unsafe_blocks)]

//! S31 cold boot: one reset root, bounded boot sequences, then stage two.

#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod profile;

#[cfg(target_arch = "riscv32")]
include!("entry.rs");

#[cfg(not(target_arch = "riscv32"))]
fn main() {}
