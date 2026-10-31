//! A board of the stand: the stand file's board ([`Board`]) and every
//! operation on it: its write through the one write operation of
//! `crate::image`, its start (the chip profile's `[flash] start`), its
//! reset ladder, its console and its hub power (`oer-lab-board`).
#![forbid(unsafe_code)]

mod board;

pub use board::{Board, BoardConsole, BoardLines, Via};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
