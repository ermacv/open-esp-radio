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

oer_portable_wire::bitfield! {
    /// The number a key's octets encode.
    /// SOURCE: postcard-schema 0.2.5, `key::hash::fnv1a64` (the hash's octets,
    /// least significant first).
    struct KeyNumber[8] (little) {
        value, with_value: {u64} = 0..64;
    }
}

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
        KeyNumber::from_bytes(self.0).value()
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

/// Declares a module's messages: their paths, how they are used and the
/// module's `KEYS`.
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
            ::serde::Serialize, ::serde::Deserialize, $crate::postcard_schema::Schema,
        )]
        #[postcard(crate = $crate::postcard_schema)]
        pub struct $name;
        impl $crate::Message for $name {
            const PATH: &'static str = $path;
        }
    };
}

#[cfg(test)]
mod tests {
    use super::{Key, Message};

    crate::messages! {
        /// A property, declared through the crate's own paths.
        property Declared = "test/declared";
    }

    #[test]
    fn a_property_is_a_message_of_its_path() {
        assert_eq!(KEYS, [Declared::KEY]);
        assert_eq!(Declared::KEY, Key::of::<Declared>("test/declared"));
        assert_ne!(Declared::KEY, Key::of::<Declared>("test/other"));
    }

    #[test]
    fn a_key_s_number_takes_its_octets_least_significant_first() {
        assert_eq!(
            Key([1, 2, 0, 0, 0, 0, 0, 3]).to_u64(),
            0x0300_0000_0000_0201
        );
    }
}
