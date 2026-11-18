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
/// Block B's frames: case C's layout, the sender's (`tasks/165`).
#[cfg(target_arch = "riscv32")]
#[path = "../../esp32s31-mac-reset/src/case_c.rs"]
#[allow(dead_code, reason = "the sender's half is esp32s31-mac-reset's")]
mod case_c;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod console_write;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod crc;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod shutdown;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod verdict;
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
