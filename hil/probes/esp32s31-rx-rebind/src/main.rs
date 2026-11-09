#![cfg_attr(target_arch = "riscv32", no_main)]
#![cfg_attr(target_arch = "riscv32", no_std)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing
)]

#[cfg(target_arch = "riscv32")]
mod diagnostic;
#[cfg(target_arch = "riscv32")]
mod image;
#[cfg(target_arch = "riscv32")]
mod phy_pilot;
#[cfg(target_arch = "riscv32")]
mod usb;
#[cfg(target_arch = "riscv32")]
use image::*;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod console_write;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod crc;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod verdict;
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
