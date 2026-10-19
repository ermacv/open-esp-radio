#![no_std]
#![forbid(unsafe_code)]

//! ESP32-C5 SoC services backed by esp-hal peripheral witnesses.
//!
//! Radio registers belong to `oer-esp32c5-pac`. This crate owns esp-hal
//! singleton witnesses for non-radio platform blocks, separate from radio
//! protocol policy.

pub mod watchdog;
