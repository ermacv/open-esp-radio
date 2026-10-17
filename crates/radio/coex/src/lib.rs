#![no_std]
#![forbid(unsafe_code)]

//! The vocabulary every radio protocol and backend shares for one RF path.
//!
//! [`RadioClient`] names a protocol that joins the shared radio, and
//! [`CoexPriority`] how urgently one of its operations needs the antenna.
//! Protocols express their own priority levels in this vocabulary; each
//! backend maps a client and a priority onto its own arbitration (event
//! numbers, hardware priority values, request kinds), which stay below the
//! backend's port. Neither value acquires anything: joining the radio and
//! requesting the antenna are operations of the backend that owns it.
//!
//! No backend reports whether the antenna was granted or denied: the
//! Espressif arbiter decides by priority in hardware and gives software no
//! acknowledgement. This crate therefore defines no grant outcome.

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

/// How urgently one operation of a client needs the shared antenna.
///
/// The levels are ordered and relative to the client: a backend maps each
/// client's levels onto its own arbitration priorities, so equal levels of
/// two clients need not win alike. A protocol that distinguishes fewer
/// levels uses a subset.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CoexPriority {
    /// No operation runs; the client keeps only its idle claim, such as a
    /// receiver left on.
    Idle,
    /// The client's ordinary priority.
    #[default]
    Normal,
    /// Above the ordinary priority.
    Elevated,
    /// The client's highest priority.
    Critical,
}

impl CoexPriority {
    /// Every level, from the lowest.
    pub const ALL: [Self; 4] = [Self::Idle, Self::Normal, Self::Elevated, Self::Critical];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priorities_order_from_idle_to_critical() {
        assert!(CoexPriority::ALL.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(CoexPriority::default(), CoexPriority::Normal);
    }

    #[test]
    fn every_client_is_listed_once() {
        for (index, client) in RadioClient::ALL.iter().enumerate() {
            assert!(!RadioClient::ALL[index + 1..].contains(client));
        }
    }
}
