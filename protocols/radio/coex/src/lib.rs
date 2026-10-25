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
    clippy::arithmetic_side_effects
)]

//! The vocabulary every radio protocol and backend shares for one RF path.
//!
//! [`RadioClient`] names a protocol that joins the shared radio, and
//! [`Operation`] marks an enum of one protocol's operations (a beacon, an
//! acknowledgement, a connection event, ...). Each protocol's port owns its
//! operation kinds; the chip family's coexistence owner maps a client and an
//! operation onto its own arbitration (event numbers, hardware priority
//! values, request kinds), which stay below the backend's port. There is no
//! urgency level: what an operation is decides how it is served. Neither
//! value acquires anything: joining the radio and requesting the antenna are
//! operations of the backend that owns it.
//!
//! No backend reports whether the antenna was granted or denied for an
//! operation served by priority alone: the Espressif arbiter decides by
//! priority in hardware and gives software no acknowledgement. This crate
//! therefore defines no grant outcome.

/// A radio protocol that shares the powered radio and its RF path.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RadioClient {
    /// IEEE 802.11.
    Wifi,
    /// Bluetooth LE.
    Bluetooth,
    /// IEEE 802.15.4.
    Ieee802154,
}

impl RadioClient {
    /// Every client, in declaration order.
    pub const ALL: [Self; 3] = [Self::Wifi, Self::Bluetooth, Self::Ieee802154];
}

/// How an operation is served on the shared antenna.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AirClass {
    /// By hardware priority alone; the operation may lose the air.
    PtiOnly,
    /// An operation with a deadline the protocol cannot move: it claims the
    /// air and waits for a grant, or, when shorter than a grant, is enforced
    /// by priority and reported as a blackout.
    HardDeadline,
}

/// One kind of operation of a [`RadioClient`].
///
/// A protocol port implements it for the enum that names what its requests
/// are.
pub trait Operation: Copy {
    /// The protocol the operation belongs to.
    const CLIENT: RadioClient;

    /// How the operation is served on the shared antenna.
    fn class(&self) -> AirClass;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_client_is_listed_once() {
        for (index, client) in RadioClient::ALL.iter().enumerate() {
            assert!(!RadioClient::ALL[index + 1..].contains(client));
        }
    }
}
