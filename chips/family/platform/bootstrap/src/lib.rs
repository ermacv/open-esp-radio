#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]
#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
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
