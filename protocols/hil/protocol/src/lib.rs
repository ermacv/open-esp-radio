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
//! Target-neutral control and evidence protocol for hardware-in-the-loop tests.
//!
//! The wire types deliberately contain no board paths, expected image hashes,
//! vendor ABI versions, or target-specific register layouts. Those belong to
//! the firmware adapter.

// The framework: message identity, framing and the envelope.
mod envelope;
mod framing;
mod key;
pub use envelope::{Envelope, WireKind};
pub use framing::{
    Addressed, DecodeCounters, DecodeError, EncodeError, FRAMING_VERSION, Frame, FrameDecoder,
    FrameEncoder, MAX_POSTCARD_BYTES, MAX_WIRE_FRAME_BYTES, RequestIdentity,
};
pub use key::{Endpoint, Key, Message};

// The modules: each is a path prefix and a directory.
pub mod base;
#[doc(hidden)]
pub use postcard_schema;
