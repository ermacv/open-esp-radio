#![cfg_attr(target_arch = "riscv32", no_std)]
#![cfg_attr(target_arch = "riscv32", no_main)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]
#![forbid(unsafe_code)]
#[cfg(all(target_arch = "riscv32", oer_b0_configured))]
include!("entry.rs");
#[cfg(all(target_arch = "riscv32", not(oer_b0_configured)))]
include!("unconfigured.rs");
#[cfg(all(test, not(target_arch = "riscv32")))]
mod gate_fault;
#[cfg(all(test, not(target_arch = "riscv32")))]
mod release_fault;
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
