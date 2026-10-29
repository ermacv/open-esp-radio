//! The vendor-versus-production PHY calibration cross-check of the
//! ESP32-S31: what is compared and how.
//!
//! The capture is the HIL `phy` family's `vendor-calibration` workload
//! (`oer-hil-family-phy`): it flashes the vendor calibration firmware and
//! the production image alternately under the run's lease and records each
//! boot as a typed observation (`oer_hil_family_phy::calibration_capture`'s `boots`, with
//! the register spaces and the vendor console protocol). This library
//! defines the ESP32-S31's register images both sides read ([`registers`])
//! and the comparison ([`compare`]) of the vendor
//! object with the production projection the firmware itself computes from
//! its retained calibration: the
//! reviewed `phy_param` relation the tracking scenario compares, with
//! `src/tolerances.toml` as the review, yielding a
//! MATCH, DIFF or INCOMPLETE [`compare::Summary`].

#![forbid(unsafe_code)]

pub mod family;

pub mod baseline;
pub mod compare;
pub mod registers;

use oer_esp32s31_knowledge::phy_relation::committed;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
