//! A board of the stand: the stand file's board ([`Board`], a description)
//! and, under the board's device access, every operation on it
//! ([`LeasedBoard`]): its write through the one write operation of
//! `oer_devices::image`, its start (the chip profile's `[flash] start`), its
//! reset ladder, its console and its hub power (`oer-stand-power`).
#![forbid(unsafe_code)]

mod board;

pub use board::{Board, BoardConsole, BoardLines, LeasedBoard, Via};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
