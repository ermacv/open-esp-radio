#![cfg_attr(target_arch = "riscv32", no_std)]
#![cfg_attr(target_arch = "riscv32", no_main)]
#![cfg_attr(
    target_arch = "riscv32",
    feature(type_alias_impl_trait, impl_trait_in_assoc_type)
)]
#![forbid(unsafe_code)]
#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
