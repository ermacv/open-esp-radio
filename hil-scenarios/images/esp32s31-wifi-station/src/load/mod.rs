//! The HIL load service (`oer-services-load`): UDP load the image receives,
//! echoes and sends on one port of its DHCP address, its counters, and the
//! CPU the executors report.
//!
//! [`action`] decides each request, [`receive`] counts received load,
//! [`pace`] schedules paced transmission, [`cpu`] accumulates the
//! executors' activity, [`parts`] the polls of the image's named parts and
//! [`counters`] the hart's and the network bridge's counts, [`sampler`]
//! where the hart spends a window; `serve` runs them on the station's IP
//! stack.

pub(crate) mod action;
pub(crate) mod counters;
pub(crate) mod cpu;
pub(crate) mod pace;
pub(crate) mod parts;
pub(crate) mod receive;
pub(crate) mod sampler;
#[cfg(target_arch = "riscv32")]
pub(crate) mod serve;
