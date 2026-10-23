#![no_std]
#![forbid(unsafe_code)]

//! Typed boot diagnostics: a transport-independent record codec and an
//! optional synchronous line frame. HIL puts the codec bytes in its own
//! frame; only the host renders text. No storage or transport is installed.

pub mod console;
pub mod fifo;
pub mod note;
pub mod path;
pub mod record;
#[cfg(feature = "render")]
pub mod render;

pub use console::{Console, DecodeError, MAX_NOTE_LINE, PREFIX, decode, emit_note};
pub use fifo::{Fifo, FifoWriter, Lost, Patience};
pub use note::{Note, Topic};
pub use path::{DiagPath, DiagnosticPath};
pub use record::{BootSmokePassed, MAX_PAYLOAD, PreviousPanic, Record};

#[cfg(test)]
extern crate std;
