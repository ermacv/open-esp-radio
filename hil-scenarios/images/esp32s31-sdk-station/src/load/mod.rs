//! The HIL load service (`oer-services-load`): UDP load the image receives,
//! echoes and sends on one port of its DHCP address, its counters, and the
//! library's observations of its own work.
//!
//! [`action`] decides each request, [`receive`] counts received load,
//! [`pace`] schedules paced transmission and [`report`] puts the library's
//! observations (`oer_espressif::observe`) in the service's vocabulary;
//! `serve` runs a run in the second boot on hart 1 beside the IP stack, and
//! `relay` serves what it needs of the boot hart: its counters and the PC
//! sampler.

pub(crate) mod action;
pub(crate) mod pace;
pub(crate) mod receive;
#[cfg(target_arch = "riscv32")]
pub(crate) mod relay;
pub(crate) mod report;
#[cfg(target_arch = "riscv32")]
pub(crate) mod serve;
