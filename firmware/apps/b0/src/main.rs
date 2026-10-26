#![cfg_attr(target_arch = "riscv32", no_std)]
#![cfg_attr(target_arch = "riscv32", no_main)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]
#![forbid(unsafe_code)]
#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(all(test, not(target_arch = "riscv32")))]
mod gate_fault;
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
