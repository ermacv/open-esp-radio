#![cfg_attr(
    target_arch = "riscv32",
    expect(
        clippy::cast_possible_truncation,
        reason = "R2 cleans a probe with the area it reads (tasks/312)"
    )
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
#![cfg_attr(target_arch = "riscv32", no_std)]
#![cfg_attr(target_arch = "riscv32", no_main)]
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
mod est_freq;
#[cfg(target_arch = "riscv32")]
mod hardware_image;
#[cfg(target_arch = "riscv32")]
#[allow(
    dead_code,
    reason = "shared session also contains the comparison-only entry and retained MAC owner"
)]
mod session;
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
