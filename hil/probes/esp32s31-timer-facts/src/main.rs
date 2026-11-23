#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
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

//! SYSTIMER facts on the stand (`tasks/380`): targets loaded into
//! comparators 1 and 2 of unit 0, whose enables were written once at boot,
//! while the executor's own alarm keeps comparator 0. The host asks for a
//! case over HIL; the image runs it and answers with its counts and times
//! in the hart's cycles. See the README.

#[cfg(target_arch = "riscv32")]
mod comparators;
#[cfg(target_arch = "riscv32")]
mod image;
#[cfg(any(target_arch = "riscv32", test))]
mod trial;
#[cfg(target_arch = "riscv32")]
#[path = "../../esp32s31-case-c-sender/src/usb.rs"]
mod usb;
#[cfg(target_arch = "riscv32")]
use image::*;
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
