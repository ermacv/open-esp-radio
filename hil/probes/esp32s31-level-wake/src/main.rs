#![cfg_attr(
    target_arch = "riscv32",
    expect(
        clippy::cast_possible_truncation,
        reason = "the probe's first change: deleted, or moved onto HIL (plan, section 6)"
    )
)]
#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

//! A level task's wake on the boot hart through the product's own parts:
//! its entry in the interrupt table, the HAL's controller and router, its
//! pend from a handler above and from an executor's task below, and the
//! stored clear of its pending byte. See the README.

#[cfg(any(target_arch = "riscv32", test))]
mod calc;

#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
