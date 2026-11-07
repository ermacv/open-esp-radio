#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![deny(clippy::panic, clippy::todo, clippy::unimplemented)]

//! The HIL image of the load scenario on the network core: the same station,
//! HIL services and load server as `hil-wifi-station`, with the network core in
//! place of the IP service, so one scenario measures both.

#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}

#[cfg(target_arch = "riscv32")]
mod services;

#[cfg(target_arch = "riscv32")]
#[path = "../../wifi-station/src/load/mod.rs"]
mod load;

#[cfg(target_arch = "riscv32")]
mod udp;
