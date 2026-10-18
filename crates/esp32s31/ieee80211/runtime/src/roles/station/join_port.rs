//! Concrete ESP32-S31 port for pre-connected STA Authentication/Association.
//!
//! RX ownership, borrowed resources, port ownership and join sequencing live
//! in separate private modules.

mod owner;
mod resources;
#[cfg(target_arch = "riscv32")]
mod rx;
mod service;

#[cfg(target_arch = "riscv32")]
pub use rx::StaJoinRx;

pub use owner::StaJoinPort;

pub use resources::{StaJoinRadio, StaJoinStation, StaJoinStorage};

#[cfg(test)]
mod tests;
