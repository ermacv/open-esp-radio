//! Message identity.
//!
//! Every message is its own type with a path such as `core/boot`. Its [`Key`]
//! hashes the path together with the type's complete wire schema, so two
//! builds give a message the same key exactly when they encode it the same
//! way. A frame carries the key of its payload's type instead of a position
//! in a shared enum: changing one message changes only that message's key.
//!
//! The schema describes the encoding, not the meaning. A change of meaning
//! that keeps the encoding (another unit in the same integer, a reused field)
//! must take a new path, and units belong in the type (a newtype per unit),
//! so that changing them changes the schema.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// A message's wire identity: FNV-1a 64 over its path and its schema.
/// Keys order as the number they encode.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, Schema)]
pub struct Key(pub [u8; 8]);

impl Ord for Key {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.to_u64().cmp(&other.to_u64())
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Key {
    /// The key of type `T` published at `path`, computed at compile time.
    pub const fn of<T: Schema + ?Sized>(path: &str) -> Self {
        Self(postcard_schema::key::hash::fnv1a64::hash_ty_path::<T>(path))
    }

    pub const fn to_u64(self) -> u64 {
        u64::from_le_bytes(self.0)
    }
}

impl core::fmt::Display for Key {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "{:016x}", self.to_u64())
    }
}

/// One message type of the protocol.
pub trait Message: Serialize + DeserializeOwned + Schema {
    /// Where the message lives: its module is the first path segment.
    const PATH: &'static str;
    const KEY: Key = Key::of::<Self>(Self::PATH);
    /// The frame direction: the host sends endpoints, the device the rest.
    const WIRE_KIND: crate::WireKind = crate::WireKind::Event;
}

/// A message the host sends and the device answers with exactly one
/// [`Endpoint::Response`], or with the `base` module's `Rejected`.
pub trait Endpoint: Message {
    type Response: Message;
}

/// How a message is used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// A request the device serves.
    Endpoint,
    /// A device message: an endpoint's response or an unsolicited event.
    Topic,
    /// A marker the device advertises among its image keys; never sent.
    Property,
}

/// The description of one message in a module's registry.
#[cfg(feature = "registry")]
#[derive(Clone, Copy)]
pub struct MessageInfo {
    pub path: &'static str,
    pub key: Key,
    pub kind: Kind,
    /// The complete wire schema, for the registry's own consistency checks.
    pub schema: &'static postcard_schema::schema::NamedType,
    /// Decodes one payload of this message to its readable form.
    pub decode_json: fn(&[u8]) -> Option<serde_json::Value>,
}

#[cfg(feature = "registry")]
impl core::fmt::Debug for MessageInfo {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("MessageInfo")
            .field("path", &self.path)
            .field("key", &self.key)
            .field("kind", &self.kind)
            .finish()
    }
}

#[cfg(feature = "registry")]
#[doc(hidden)]
pub fn decode_json<M: Message>(payload: &[u8]) -> Option<serde_json::Value> {
    let message: M = postcard::from_bytes(payload).ok()?;
    serde_json::to_value(message).ok()
}

/// Declares a module's messages: their paths, how they are used, and (with
/// the `registry` feature) the module's `MESSAGES` list.
///
/// ```ignore
/// messages! {
///     endpoint GetBootStatus = "core/boot/get" => BootEvidence;
///     topic BootEvidence = "core/boot";
///     property UdpMultiFlow = "network/udp-multi-flow";
/// }
/// ```
///
/// An endpoint and a topic name an existing type; a property declares its
/// own unit marker type.
#[macro_export]
macro_rules! messages {
    ($($(#[$meta:meta])* $kind:ident $name:ident = $path:literal $(=> $response:ty)?;)*) => {
        $($crate::__message!($(#[$meta])* $kind $name = $path $(=> $response)?);)*

        /// This module's messages.
        #[cfg(feature = "registry")]
        pub const MESSAGES: &[$crate::MessageInfo] = &[
            $($crate::MessageInfo {
                path: $path,
                key: <$name as $crate::Message>::KEY,
                kind: $crate::__message_kind!($kind),
                schema: <$name as $crate::__Schema>::SCHEMA,
                decode_json: $crate::__decode_json::<$name>,
            },)*
        ];
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! __message {
    ($(#[$meta:meta])* endpoint $name:ident = $path:literal => $response:ty) => {
        $(#[$meta])*
        impl $crate::Message for $name {
            const PATH: &'static str = $path;
            const WIRE_KIND: $crate::WireKind = $crate::WireKind::Command;
        }
        $(#[$meta])*
        impl $crate::Endpoint for $name {
            type Response = $response;
        }
    };
    ($(#[$meta:meta])* topic $name:ident = $path:literal) => {
        $(#[$meta])*
        impl $crate::Message for $name {
            const PATH: &'static str = $path;
        }
    };
    ($(#[$meta:meta])* property $name:ident = $path:literal) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, Eq, PartialEq,
            ::serde::Serialize, ::serde::Deserialize, $crate::__SchemaDerive,
        )]
        #[postcard(crate = $crate::__postcard_schema)]
        pub struct $name;
        impl $crate::Message for $name {
            const PATH: &'static str = $path;
        }
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! __message_kind {
    (endpoint) => {
        $crate::Kind::Endpoint
    };
    (topic) => {
        $crate::Kind::Topic
    };
    (property) => {
        $crate::Kind::Property
    };
}
