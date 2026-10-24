#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! ESP32-C5 SoC services backed by esp-hal peripheral witnesses.
//!
//! Radio registers belong to `oer-esp32c5-pac`. This crate consumes esp-hal
//! singleton witnesses and transfers their blocks: [`handoff`] the entropy,
//! reset-cause, watchdog and USB Serial/JTAG blocks, [`radio`] the radio
//! blocks.

#[cfg(feature = "esp32c5")]
pub mod handoff;

#[cfg(feature = "esp32c5")]
pub mod radio;
