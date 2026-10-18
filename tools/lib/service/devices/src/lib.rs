//! Operations on attached development boards: discovery, serial ports,
//! console captures, resets and OpenOCD.
//!
//! With the `image` feature, `devices` finds boards by port, MAC and known
//! chip, and `Opened` holds a board's device lock for image writes,
//! starts, resets and console sessions.
//!
//! A write goes through `image::write`,
//! which invalidates the board's receipt before the flash changes and
//! publishes the whole bundle's receipt after; `cargo fw` therefore leaves
//! no stale image state for the stand or HIL, without depending on them.
//!
//! These operations are modules of this package. Only the device identity,
//! lock and reference-peer line grammar have separate packages.
#![forbid(unsafe_code)]

pub mod console;
pub mod discovery;
pub mod openocd;
pub mod port;
pub mod reset;

#[cfg(feature = "image")]
pub mod device;
#[cfg(feature = "image")]
pub mod flash;
#[cfg(feature = "image")]
pub mod image;

#[cfg(feature = "image")]
pub use device::{Console, Device, Opened, devices, find};
#[cfg(feature = "image")]
pub use image::Receipt;
pub use oer_device_lock::DeviceId;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
