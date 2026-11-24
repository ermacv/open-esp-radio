#![cfg_attr(
    target_arch = "riscv32",
    expect(
        clippy::cast_possible_truncation,
        reason = "the probe's first change: deleted, or moved onto HIL (plan, section 6)"
    )
)]
#![cfg_attr(
    target_arch = "riscv32",
    expect(
        clippy::cast_sign_loss,
        reason = "the probe's first change: deleted, or moved onto HIL (plan, section 6)"
    )
)]
#![cfg_attr(
    target_arch = "riscv32",
    expect(
        clippy::let_underscore_must_use,
        reason = "the probe's first change: deleted, or moved onto HIL (plan, section 6)"
    )
)]
#![cfg_attr(target_arch = "riscv32", no_std)]
#![cfg_attr(target_arch = "riscv32", no_main)]
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
#![cfg_attr(
    target_arch = "riscv32",
    expect(
        clippy::arithmetic_side_effects,
        reason = "the probe's first change: deleted, or moved onto HIL (plan, section 6)"
    )
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
