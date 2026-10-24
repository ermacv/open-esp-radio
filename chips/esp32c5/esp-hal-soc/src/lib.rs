#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! ESP32-C5 SoC services backed by esp-hal peripheral witnesses.
//!
//! Radio registers belong to `oer-esp32c5-pac`. This crate owns esp-hal
//! singleton witnesses for non-radio platform blocks, separate from radio
//! protocol policy. [`handoff`] transfers the entropy and reset-cause blocks
//! to the chip PAC.

#[cfg(feature = "esp32c5")]
pub mod handoff;
pub mod watchdog;

#[cfg(feature = "esp32c5")]
pub mod radio;
