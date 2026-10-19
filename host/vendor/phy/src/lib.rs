//! The host half of the PHY vendor calibration cross-check: the chip's
//! comparison port ([`comparison::Comparison`]) and the driver of the vendor
//! IEEE 802.15.4 reference firmware ([`reference`](mod@reference)), which reads the vendor
//! side's register state. The family names no chip: a chip-specific
//! composition implements the port.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod comparison;
pub mod reference;

pub(crate) use oer_hil_workload::Result;
