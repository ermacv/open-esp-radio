#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![deny(clippy::panic, clippy::todo, clippy::unimplemented)]

//! The HIL image of the radio-free scenarios: the `oer-espressif` AES
//! service with the library's defaults, the HIL base module and the AES
//! service over the USB Serial/JTAG console, without Wi-Fi.

#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}

#[cfg(any(target_arch = "riscv32", test))]
mod services;
