#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![deny(clippy::panic, clippy::todo, clippy::unimplemented)]

//! The HIL image of the Wi-Fi access-point scenario: the `oer` Wi-Fi
//! access point and its network, the HIL base module and the
//! access-point service over the USB Serial/JTAG console.

#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}

#[cfg(any(target_arch = "riscv32", test))]
mod services;
