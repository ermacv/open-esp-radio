//! The HIL load service (`oer-hil-services-load`): UDP load the image receives,
//! echoes and sends on the image's UDP (`crate::udp`), its counters, and the
//! library's observations of its own work.
//!
//! [`action`] decides each request, [`receive`] counts received load,
//! [`pace`] schedules paced transmission and [`report`] puts the library's
//! observations (`oer::observe`) in the service's vocabulary;
//! `serve` runs a run in the group on hart 1 beside the network, and
//! `relay` serves what it needs of the boot hart: its counters and the PC
//! sampler.

pub(crate) mod action;
pub(crate) mod barrier;
pub(crate) mod pace;
pub(crate) mod receive;
#[cfg(target_arch = "riscv32")]
pub(crate) mod relay;
pub(crate) mod report;
#[cfg(target_arch = "riscv32")]
pub(crate) mod serve;
