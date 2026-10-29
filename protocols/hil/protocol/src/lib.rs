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
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods
    )
)]
//! Target-neutral control and evidence protocol for hardware-in-the-loop tests.
//!
//! The wire types deliberately contain no board paths, expected image hashes,
//! vendor ABI versions, or target-specific register layouts. Those belong to
//! the firmware adapter.

pub mod trace_header;

// The framework: message identity, framing and the envelope.
mod envelope;
mod framing;
#[cfg(feature = "async-io")]
mod io;
mod key;
pub use framing::{
    DecodeCounters, DecodeError, EncodeError, FRAMING_VERSION, Frame, FrameDecoder, FrameEncoder,
    MAX_POSTCARD_BYTES, MAX_WIRE_FRAME_BYTES, Outbound, RequestIdentity,
};
#[cfg(feature = "async-io")]
pub use io::{WriteFrameError, write_frame};
#[cfg(feature = "descriptors")]
pub use key::{LockMismatch, MessageInfo, check_lock};

/// This crate's reviewed framing version, message paths, keys and kinds,
/// checked against its descriptors by this crate's own test; every other
/// message owner checks its own lock the same way ([`check_lock`]).
#[cfg(feature = "descriptors")]
pub const MESSAGES_LOCK: &str = include_str!("../messages.lock");
pub use envelope::{Envelope, WireKind};
pub use key::{Endpoint, Key, Kind, Message};

// The modules: each is a path prefix and a directory.
pub mod base;
pub mod capture;
pub mod lifecycle;
pub mod system;

#[cfg(all(test, feature = "descriptors"))]
mod lock_tests;

/// The framework's own descriptors, by module.
#[cfg(feature = "descriptors")]
pub const DESCRIPTORS: &[&[MessageInfo]] =
    &[base::MESSAGES, lifecycle::MESSAGES, capture::MESSAGES];
#[doc(hidden)]
pub use postcard_schema as __postcard_schema;
#[doc(hidden)]
pub use postcard_schema::Schema as __Schema;
#[doc(hidden)]
pub use postcard_schema::Schema as __SchemaDerive;
