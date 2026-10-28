#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing
)]
//! The ESP32-S31 side of an image's platform over the Espressif staged
//! runtime (`oer-espressif-staged-runtime`), whose fatal paths run on the
//! HAL's port ([`oer_esp32s31_hal::platform::Port`]).
//!
//! It holds the chip's retained fault records, which need both the HAL's
//! types and the runtime's retained memory and fatal path, so neither of
//! them can hold them:
//!
//! - [`phy_fault`]: the PHY transaction seat, its retained fault record, the
//!   reset the HAL's `phy_fault_reporter!` names and the next boot's report,
//!   which the image passes to `Handoff::adopt`;
//! - `wifi_fault` (feature `wifi-fault`): the Wi-Fi MAC's fatal assertion
//!   record, its reset and the next boot's report.

pub mod phy_fault;
/// Shared retained/reset path selected only by staged Wi-Fi images.
#[cfg(feature = "wifi-fault")]
pub mod wifi_fault;
