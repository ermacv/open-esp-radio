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

/// The description of one message in a module's descriptors.
#[cfg(feature = "descriptors")]
#[derive(Clone, Copy)]
pub struct MessageInfo {
    pub path: &'static str,
    pub key: Key,
    pub kind: Kind,
    /// The complete wire schema.
    pub schema: &'static postcard_schema::schema::NamedType,
}

#[cfg(feature = "descriptors")]
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

/// Where an owner's reviewed `messages.lock` and its descriptors disagree.
#[cfg(feature = "descriptors")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockMismatch<'a> {
    /// The lock's framing line is not this crate's [`crate::FRAMING_VERSION`].
    Framing,
    /// A lock line is not `path key kind` of the `[messages]` section.
    Line(&'a str),
    /// A lock line names no descriptor, or another key or kind.
    Stale(&'a str),
    /// A descriptor the lock does not list.
    Missing(&'static str),
}

/// Check `lock`, an owner's reviewed `messages.lock`, against its
/// `descriptors`: the framing version and exactly one `path key kind` line
/// per descriptor. Comment lines and the `[modules]` section are reviewed
/// text and not compared.
#[cfg(feature = "descriptors")]
pub fn check_lock<'a>(lock: &'a str, descriptors: &[&[MessageInfo]]) -> Result<(), LockMismatch<'a>> {
    let mut framing = false;
    let mut messages = false;
    let mut listed = 0_usize;
    for line in lock.lines().filter(|line| !line.is_empty() && !line.starts_with('#')) {
        if let Some(version) = line.strip_prefix("framing ") {
            if version.parse::<u8>() != Ok(crate::FRAMING_VERSION) {
                return Err(LockMismatch::Framing);
            }
            framing = true;
            continue;
        }
        if line.starts_with('[') {
            messages = line == "[messages]";
            continue;
        }
        if !messages {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(path), Some(key), Some(kind), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(LockMismatch::Line(line));
        };
        let kind = match kind {
            "endpoint" => Kind::Endpoint,
            "topic" => Kind::Topic,
            "property" => Kind::Property,
            _ => return Err(LockMismatch::Line(line)),
        };
        let matches = descriptors.iter().copied().flatten().filter(|info| {
            info.path == path
                && info.kind == kind
                && key.len() == 16
                && u64::from_str_radix(key, 16) == Ok(info.key.to_u64())
        });
        if matches.count() != 1 {
            return Err(LockMismatch::Stale(line));
        }
        listed = listed.checked_add(1).ok_or(LockMismatch::Line(line))?;
    }
    if !framing {
        return Err(LockMismatch::Framing);
    }
    for info in descriptors.iter().copied().flatten() {
        let mut lines = lock.lines();
        if !lines.any(|line| line.split_whitespace().next() == Some(info.path)) {
            return Err(LockMismatch::Missing(info.path));
        }
    }
    let total = descriptors
        .iter()
        .try_fold(0_usize, |total, module| total.checked_add(module.len()));
    if total == Some(listed) {
        Ok(())
    } else {
        Err(LockMismatch::Line("duplicate message line"))
    }
}

/// Declares a module's messages: their paths, how they are used, the
/// module's `KEYS` and (with the calling crate's `descriptors` feature) its
/// `MESSAGES` list.
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

        /// The keys of this module's messages, in declaration order.
        #[allow(unused_doc_comments)]
        pub const KEYS: &[$crate::Key] = &[$($(#[$meta])* <$name as $crate::Message>::KEY,)*];

        /// This module's messages.
        #[cfg(feature = "descriptors")]
        pub const MESSAGES: &[$crate::MessageInfo] = &[
            $($crate::MessageInfo {
                path: $path,
                key: <$name as $crate::Message>::KEY,
                kind: $crate::__message_kind!($kind),
                schema: <$name as $crate::__Schema>::SCHEMA,
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
