#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods
    )
)]

//! Typed boot diagnostics: a transport-independent record codec and an
//! optional synchronous line frame. HIL puts the codec bytes in its own
//! frame; only the host renders text. No storage or transport is installed.

pub mod console;
pub mod fault;
pub mod fifo;
pub mod note;
pub mod path;
pub mod radio_root;
pub mod record;
#[cfg(feature = "render")]
pub mod render;
pub mod text;

pub use console::{
    Console, DecodeError, MAX_NOTE_LINE, PREFIX, decode, emit_note, emit_previous_panic,
};
pub use fifo::{Fifo, FifoWriter, Lost, Patience};
pub use note::{Note, Topic};
pub use path::{DiagPath, DiagnosticPath};
pub use record::{
    BootSmokePassed, HartAes, HartExchange, HartLiveness, MAX_PAYLOAD, PreviousPanic, Record,
    SecondaryReady,
};
pub use text::Name;

#[cfg(test)]
extern crate std;
