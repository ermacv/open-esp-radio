//! The capture of the vendor-versus-production PHY calibration
//! cross-check, chip-neutral: what the HIL `phy` family
//! (`oer-hil-family-phy`) records and a chip's comparison reads.
//!
//! [`boots`] holds the typed captures (each vendor and production boot, the
//! lifecycle point, the images both sides ran), [`space`] the register
//! spaces both sides read, and [`vendor`] the vendor firmware's console
//! protocol. What a chip compares, and the register images it reads, are
//! the chip's comparison (`oer-esp32s31-phy-vendor-calibration` for the
//! ESP32-S31), which the HIL family reaches through its comparison port.
#![forbid(unsafe_code)]

pub mod boots;
pub mod space;
pub mod vendor;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
