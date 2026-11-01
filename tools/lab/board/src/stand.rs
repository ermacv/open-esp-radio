//! A board of the stand: the stand file's board ([`Board`]: its console, its
//! description and its hub power, for any chip) and, for a chip with a
//! profile ([`ProfiledBoard`]), every other operation on it: its write
//! through the one write operation of `crate::image`, its start (the chip
//! profile's `[flash] start`), its RTS, JTAG and download resets and its
//! reset ladder (`oer-lab-board`).
#![forbid(unsafe_code)]

mod board;

pub use board::{Board, BoardConsole, BoardLines, ProfiledBoard, Via};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
