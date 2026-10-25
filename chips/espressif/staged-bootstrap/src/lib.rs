#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
//! Staged bootstrap entry, application descriptor and payload relocation.
mod app_descriptor;
pub use app_descriptor::{AppDescriptor, DescriptorError};
#[cfg(target_arch = "riscv32")]
mod runtime;
#[cfg(target_arch = "riscv32")]
pub use runtime::*;
#[cfg(target_arch = "riscv32")]
#[allow(unsafe_code, reason = "reset entry before the Rust stack exists")]
mod entry;
