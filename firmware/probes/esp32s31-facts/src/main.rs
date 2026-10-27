#![cfg_attr(target_arch = "riscv32", no_std)]
#![cfg_attr(target_arch = "riscv32", no_main)]
#![deny(
    unsafe_code,
    clippy::undocumented_unsafe_blocks,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing
)]

#[cfg(target_arch = "riscv32")]
mod image;
#[cfg(any(test, target_arch = "riscv32"))]
#[allow(unsafe_code, reason = "the probe owns its trap entry/exit ABI")]
mod trap;

#[cfg(not(target_arch = "riscv32"))]
fn main() {}
