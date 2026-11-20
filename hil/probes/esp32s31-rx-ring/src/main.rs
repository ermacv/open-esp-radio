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

//! The receive ring's atoms on the stand (`tasks/277`, `281`): over the
//! radio domain the probe holds, case C's frames from the sender served by
//! the product's ring. See the README.

#[path = "../../esp32s31-mac-reset/src/case_c.rs"]
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "the sender's half is esp32s31-mac-reset's")
)]
mod case_c;
#[cfg(target_arch = "riscv32")]
#[path = "../../esp32s31-mac-reset/src/diagnostic.rs"]
mod diagnostic;
#[cfg(target_arch = "riscv32")]
mod image;
#[cfg(target_arch = "riscv32")]
#[path = "../../esp32s31-mac-reset/src/phy_pilot.rs"]
mod phy_pilot;
#[cfg(target_arch = "riscv32")]
#[path = "../../esp32s31-mac-reset/src/radio.rs"]
mod radio;
#[cfg(target_arch = "riscv32")]
#[path = "../../esp32s31-mac-reset/src/usb.rs"]
mod usb;
#[cfg(target_arch = "riscv32")]
use image::*;
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
