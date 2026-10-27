#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]
#[cfg(all(target_arch = "riscv32", oer_w3_configured))]
include!("entry.rs");
#[cfg(all(target_arch = "riscv32", not(oer_w3_configured)))]
include!("unconfigured.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
