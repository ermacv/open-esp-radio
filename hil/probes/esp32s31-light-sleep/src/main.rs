#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]

//! PMU light sleep of the S31's HP system until an LP-timer target, with
//! no domain powered down: whether the request wakes or is rejected, the
//! wake causes, and the LP timer and the system timer across each sleep.
//! See the README.

#[cfg(any(target_arch = "riscv32", test))]
mod calc;

#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
