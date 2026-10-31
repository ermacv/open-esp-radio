//! The HIL load service (`oer-services-load`): UDP load the image receives,
//! echoes and sends on one port of its DHCP address, its counters, and the
//! CPU the executors report.
//!
//! [`action`] decides each request, [`receive`] counts received load,
//! [`pace`] schedules paced transmission, [`cpu`] accumulates the
//! executors' activity and [`parts`] the polls of the image's named parts;
//! `serve` runs them on the station's IP stack.

pub(crate) mod action;
pub(crate) mod cpu;
pub(crate) mod pace;
pub(crate) mod parts;
pub(crate) mod receive;
#[cfg(target_arch = "riscv32")]
pub(crate) mod serve;
