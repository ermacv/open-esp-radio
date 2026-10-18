//! Role-local protocol runtimes.
//!
//! These modules own STA/AP/scan/monitor policy. Physical RX, TX and IRQ
//! arbitration stays in [`crate::datapath`]; concurrent composition may lend
//! capabilities to roles but does not duplicate their protocol state.

pub mod access_point;
pub mod concurrent;
pub mod esp_now;
pub mod monitor;
#[cfg(target_arch = "riscv32")]
pub mod radio_channel;
pub mod scan;
pub mod station;
