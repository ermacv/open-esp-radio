#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![deny(unsafe_code)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]

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
