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
pub use envelope::{Envelope, WireKind};
pub use framing::{
    DecodeCounters, DecodeError, EncodeError, FRAMING_VERSION, Frame, FrameDecoder, FrameEncoder,
    MAX_POSTCARD_BYTES, MAX_WIRE_FRAME_BYTES, Outbound, RequestIdentity,
};
#[cfg(feature = "async-io")]
pub use io::{WriteFrameError, write_frame};
pub use key::{Endpoint, Key, Message};

// The modules: each is a path prefix and a directory.
pub mod base;
pub mod capture;
pub mod lifecycle;
pub mod system;
#[doc(hidden)]
pub use postcard_schema as __postcard_schema;
#[doc(hidden)]
pub use postcard_schema::Schema as __SchemaDerive;
