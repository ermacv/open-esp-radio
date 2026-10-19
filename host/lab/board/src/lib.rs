//! The one path to a stand board: discovery, serial ports, console
//! captures, resets, OpenOCD, the device lock (`lock`), hub power
//! (`power`), the reference peers' line grammar (`peer_line`) and the
//! stand file's board with every operation on it (`stand`). `devices`
//! finds boards by port, MAC and known chip, and `Opened` holds a board's
//! device lock for image writes, starts, resets and console sessions.
//!
//! A write goes through `image::write`,
//! which invalidates the board's receipt before the flash changes and
//! publishes the whole bundle's receipt after; `cargo fw` therefore leaves
//! no stale image state for the stand or HIL, without depending on them.

#![forbid(unsafe_code)]

pub mod lock;
pub mod peer_line;
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

pub use crate::lock::DeviceId;
pub use device::{Console, Device, Opened, devices, find};
pub use image::Receipt;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
