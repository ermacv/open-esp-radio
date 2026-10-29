#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![deny(clippy::panic, clippy::todo, clippy::unimplemented)]
#![recursion_limit = "256"]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]
#[cfg(any(target_arch = "riscv32", test))]
mod startup;
#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}

#[cfg(all(test, not(target_arch = "riscv32")))]
mod services;
