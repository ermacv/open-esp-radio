#![expect(clippy::cast_possible_truncation, reason = "the rows no step rewrites")]
#![cfg_attr(
    test,
    expect(
        clippy::let_underscore_must_use,
        reason = "test code: a cast or a dropped result in a test loses no product's value"
    )
)]
#![no_std]
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
    clippy::indexing_slicing,
    reason = "the rows no step rewrites"
)]
//! Staged bootstrap entry, application descriptor and payload relocation.
#[cfg(test)]
extern crate std;

mod app_descriptor;
pub use app_descriptor::{AppDescriptor, DescriptorError};
mod runtime;
pub mod startup;
pub use runtime::*;
#[cfg(target_arch = "riscv32")]
#[allow(unsafe_code, reason = "reset entry before the Rust stack exists")]
mod entry;
