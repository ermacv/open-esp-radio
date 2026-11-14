#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![deny(clippy::panic, clippy::todo, clippy::unimplemented)]

//! The HIL image of the station scenarios: the `oer` Wi-Fi station with the
//! network core, the HIL base module and the SDK
//! station, Wi-Fi station, fault and load services over the USB Serial/JTAG
//! console.

#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}

#[cfg(any(target_arch = "riscv32", test))]
mod services;

#[cfg(any(target_arch = "riscv32", test))]
mod load;
#[cfg(target_arch = "riscv32")]
mod measure;

#[cfg(target_arch = "riscv32")]
mod udp;

#[cfg(target_arch = "riscv32")]
mod waits;
