//! The one path to a stand board: discovery, serial ports, console
//! captures, resets, OpenOCD, hub power (`power`) and the stand file's
//! board with every operation on it (`stand`). What is known of a board
//! apart from its hardware (reset paths, the ladder, ROM lines, the
//! reference peers' line grammar) is the stand model's
//! (`oer_lab_model::rom`, `oer_lab_model::peer_line`). `devices` finds boards by port, MAC and known
//! chip. Only the LAB service (`oer-lab-service`), the one owner of the
//! stand's hardware, runs these operations.
//!
//! A write goes through `image::write`,
//! which invalidates the board's receipt before the flash changes and
//! publishes the whole bundle's receipt after; `cargo fw` therefore leaves
//! no stale image state for the stand or HIL, without depending on them.

#![forbid(unsafe_code)]

pub mod power;
pub mod stand;

pub mod console;
pub mod discovery;
pub mod openocd;
pub mod port;
pub mod reset;

pub mod device;
pub mod flash;
pub mod image;

pub use device::{Device, devices, find};
pub use image::Receipt;
pub use oer_lab_model::device_id::DeviceId;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
