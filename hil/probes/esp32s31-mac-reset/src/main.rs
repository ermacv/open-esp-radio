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

//! The Wi-Fi MAC's block reset (`RST_WIFIMAC`) mid-run, a second run after
//! it, the key table across it, and how long an ordinary stop takes, over
//! the radio domain the probe holds itself (`tasks/084`). See the README.

#[cfg(any(test, feature = "amsdu", feature = "settle"))]
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "the receiver's half is esp32s31-rx-chain's")
)]
mod case_c;
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
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
