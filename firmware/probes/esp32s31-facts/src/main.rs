#![cfg_attr(target_arch = "riscv32", no_std)]
#![cfg_attr(target_arch = "riscv32", no_main)]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

#[cfg(target_arch = "riscv32")]
mod image;

#[cfg(not(target_arch = "riscv32"))]
fn main() {}
