//! The HIL load service (`oer-services-load`): UDP load the image receives,
//! echoes and sends on one port of its DHCP address, its counters, and the
//! CPU the executors of both harts report.
//!
//! [`action`] decides each request, [`receive`] counts received load,
//! [`pace`] schedules paced transmission, [`cpu`] accumulates the
//! executors' activity, [`parts`] the polls of the image's named parts and
//! [`counters`] the boot hart's and the network bridge's counts, [`sampler`]
//! where the boot hart spends a window; `serve` runs them on hart 1 beside
//! the station's IP stack, and `relay` carries what it needs from the boot
//! hart: the console, the sampler and the boot hart's counters.

pub(crate) mod action;
pub(crate) mod counters;
pub(crate) mod cpu;
pub(crate) mod pace;
pub(crate) mod parts;
pub(crate) mod receive;
#[cfg(target_arch = "riscv32")]
pub(crate) mod relay;
pub(crate) mod sampler;
#[cfg(target_arch = "riscv32")]
pub(crate) mod serve;
