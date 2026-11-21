#![expect(
    clippy::cast_possible_truncation,
    reason = "R2 cleans a probe with the area it reads (tasks/312)"
)]
#![cfg_attr(
    target_arch = "riscv32",
    expect(
        clippy::cast_sign_loss,
        reason = "R2 cleans a probe with the area it reads (tasks/312)"
    )
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
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]
#![expect(
    clippy::arithmetic_side_effects,
    reason = "R2 cleans a probe with the area it reads (tasks/323)"
)]

//! Case C's sender (`tasks/154`, `159`, cell E of `161`): frames to the
//! peer it learns, on the receive ring's atoms over the radio domain the
//! probe holds itself. See the README.

#[cfg_attr(
    not(test),
    allow(dead_code, reason = "the receiver's half is esp32s31-rx-ring's")
)]
mod case_c;
#[cfg(target_arch = "riscv32")]
mod diagnostic;
#[cfg(target_arch = "riscv32")]
mod image;
#[cfg(target_arch = "riscv32")]
mod phy_pilot;
#[cfg(target_arch = "riscv32")]
mod radio;
#[cfg(target_arch = "riscv32")]
mod usb;
#[cfg(target_arch = "riscv32")]
use image::*;
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
