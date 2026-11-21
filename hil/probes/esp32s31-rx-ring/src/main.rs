#![expect(
    clippy::cast_possible_truncation,
    reason = "R2 cleans a probe with the area it reads (tasks/312)"
)]
#![cfg_attr(
    target_arch = "riscv32",
    expect(
        clippy::let_underscore_must_use,
        reason = "R2 cleans a probe with the area it reads (tasks/312)"
    )
)]
#![cfg_attr(target_arch = "riscv32", no_main)]
#![cfg_attr(target_arch = "riscv32", no_std)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![expect(
    clippy::arithmetic_side_effects,
    reason = "R2 cleans a probe with the area it reads (tasks/323)"
)]

//! The receive ring's atoms on the stand (`tasks/277`, `281`): over the
//! radio domain the probe holds, case C's frames from the sender served by
//! the product's ring. See the README.

#[path = "../../esp32s31-case-c-sender/src/case_c.rs"]
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "the sender's half is esp32s31-case-c-sender's")
)]
mod case_c;
#[cfg(target_arch = "riscv32")]
#[path = "../../esp32s31-case-c-sender/src/diagnostic.rs"]
mod diagnostic;
#[cfg(target_arch = "riscv32")]
mod image;
#[cfg(target_arch = "riscv32")]
#[path = "../../esp32s31-case-c-sender/src/phy_pilot.rs"]
mod phy_pilot;
#[cfg(target_arch = "riscv32")]
#[path = "../../esp32s31-case-c-sender/src/radio.rs"]
mod radio;
#[cfg(target_arch = "riscv32")]
#[path = "../../esp32s31-case-c-sender/src/usb.rs"]
mod usb;
#[cfg(target_arch = "riscv32")]
use image::*;
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
