//! The host half of the PHY vendor calibration cross-check: the chip's
//! comparison port ([`comparison::Comparison`]) and the driver of the vendor
//! IEEE 802.15.4 reference firmware ([`reference`](mod@reference)), which reads the vendor
//! side's register state. The family names no chip: a chip-specific
//! composition implements the port.
#![forbid(unsafe_code)]

pub mod archive;
pub mod calibration_capture;

pub mod comparison;
pub mod reference;

pub(crate) use oer_hil_scenario::workload::Result;
