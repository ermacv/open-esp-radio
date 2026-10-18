#![no_std]
//! Target-neutral control and evidence protocol for hardware-in-the-loop tests.
//!
//! The wire types deliberately contain no board paths, expected image hashes,
//! vendor ABI versions, or target-specific register layouts. Those belong to
//! the firmware adapter.

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
pub use io::write_frame;
#[cfg(feature = "registry")]
pub use key::MessageInfo;

// The host's view of what an image serves, assembled from its Hello and
// image key pages.
#[cfg(feature = "registry")]
mod image_keys;
#[cfg(feature = "registry")]
pub use image_keys::DeviceImageKeys;

/// The reviewed wire of this revision: the framing version and every
/// message's path, key and kind, one per line. The registry test keeps it
/// equal to the registry; two revisions whose locks are equal speak the same
/// protocol, so one's runner drives the other's images.
#[cfg(feature = "registry")]
pub const MESSAGES_LOCK: &str = include_str!("../messages.lock");
pub use envelope::{Envelope, WireKind};
pub use key::{Endpoint, Key, Kind, Message};

// The modules: each is a path prefix and a directory.
pub mod base;
pub mod network;
pub mod system;
pub mod telemetry;

/// Every module's messages, for a host that decodes any frame.
#[cfg(feature = "registry")]
pub fn registry() -> impl Iterator<Item = &'static MessageInfo> {
    [
        base::MESSAGES,
        system::MESSAGES,
        network::MESSAGES,
        telemetry::MESSAGES,
    ]
    .into_iter()
    .flatten()
}

#[cfg(feature = "registry")]
#[doc(hidden)]
pub use key::decode_json as __decode_json;
#[doc(hidden)]
pub use postcard_schema as __postcard_schema;
#[doc(hidden)]
pub use postcard_schema::Schema as __Schema;
#[doc(hidden)]
pub use postcard_schema::Schema as __SchemaDerive;
