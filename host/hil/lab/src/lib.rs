//! HIL stand operation at the level of a run: the laboratory configuration
//! a run reads from the stand file, the lock a run holds on its boards and
//! fixtures, the pre-run observation of the cell, fixture software leases,
//! the recovery of a board that stopped answering and the post-mortem of a
//! failed repetition. The stand implements the link's `Dut` port for the
//! board it leased.
//!
//! It actuates no hardware and writes no flash record itself: boards are
//! reached through board I/O (`oer-hil-board`), exclusion and the board
//! journal belong to the arbiter, flashing to the flash operation.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod config;
mod dut;
pub use dut::{attach_console, peer_console};
mod error;
pub use error::Error;
pub mod lock;
pub mod post_mortem;
pub mod provenance;
pub mod recovery;
pub mod software;
pub mod usb_events;
pub mod wifi;

/// The root-owned laptop radio helper installed by
/// `cargo stand fixture install --provider linux-net`.
pub const NETWORK_HELPER: &str = "/usr/local/sbin/open-radio-net";

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
