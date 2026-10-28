#![cfg_attr(target_arch = "riscv32", no_main, no_std)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]

//! S31 cold boot: one reset root, bounded boot sequences, then stage two.

#[cfg(target_arch = "riscv32")]
include!("entry.rs");

#[cfg(not(target_arch = "riscv32"))]
fn main() {}
