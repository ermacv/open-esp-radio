#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![recursion_limit = "256"]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]
#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}

#[cfg(test)]
#[path = "wifi_station/security_report.rs"]
mod security_report;
