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
mod usb;
#[cfg(target_arch = "riscv32")]
use image::*;
#[cfg(all(test, not(target_arch = "riscv32")))]
#[path = "irq120/journal.rs"]
mod irq120_journal;
#[cfg(all(test, not(target_arch = "riscv32")))]
#[path = "traffic/journal.rs"]
mod traffic_journal;
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
