#![expect(
    clippy::cast_possible_truncation,
    reason = "R3 cleans the cores and the contracts (tasks/312)"
)]
#![cfg_attr(
    feature = "render",
    expect(
        clippy::cast_possible_wrap,
        reason = "R3 cleans the cores and the contracts (tasks/312)"
    )
)]
#![expect(
    clippy::cast_sign_loss,
    reason = "R3 cleans the cores and the contracts (tasks/312)"
)]
#![expect(
    clippy::let_underscore_must_use,
    reason = "R3 cleans the cores and the contracts (tasks/312)"
)]
#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

//! Typed boot diagnostics: a transport-independent record codec and an
//! optional synchronous line frame. HIL puts the codec bytes in its own
//! frame; only the host renders text. No storage or transport is installed.

pub mod console;
pub mod fault;
pub mod fifo;
pub mod line;
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
pub use fifo::{Fifo, FifoWriter, Lost, Patience, Written};
pub use line::{Format, FormatError, Hex, Ipv4, Line, LineValue, LineValues};
pub use note::{Note, Topic};
pub use path::{DiagPath, DiagnosticPath};
pub use record::{
    BootSmokePassed, HartAes, HartExchange, HartLiveness, MAX_PAYLOAD, PreviousPanic, Record,
    SecondaryReady,
};
pub use text::Name;

#[cfg(test)]
extern crate std;
