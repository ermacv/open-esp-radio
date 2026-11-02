#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]

//! The RISC-V cycle counter (`mcycle`) against the system timer on the S31:
//! one read's cost beside one read of the image's clock, its rate on each
//! hart, whether it counts while a hart waits for an interrupt, and the
//! offset between hart 0's and hart 1's counters at the same instant and
//! over a second. See the README.

#[cfg(any(target_arch = "riscv32", test))]
mod calc;

#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
