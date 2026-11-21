#![cfg_attr(
    target_arch = "riscv32",
    expect(
        clippy::cast_possible_truncation,
        reason = "R2 cleans a probe with the area it reads (tasks/312)"
    )
)]
#![cfg_attr(target_arch = "riscv32", no_std)]
#![cfg_attr(target_arch = "riscv32", no_main)]
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
        reason = "R2 cleans a probe with the area it reads (tasks/323)"
    )
)]

#[cfg(target_arch = "riscv32")]
mod image;
#[cfg(any(test, target_arch = "riscv32"))]
#[allow(unsafe_code, reason = "the probe owns its trap entry/exit ABI")]
mod trap;

#[cfg(not(target_arch = "riscv32"))]
fn main() {}
