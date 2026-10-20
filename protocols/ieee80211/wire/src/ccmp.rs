//! CCMP key material, packet-number values and eight-byte header codecs.
//!
//! Packet-number allocation and receive replay frontiers belong to the
//! upper-MAC core. This module validates values and encodes or parses their
//! wire representation without retaining an exchange's mutable state.
//!
//! [`Ccmp128Key`] is the shared zeroizing temporal-key owner. RSN constructs
//! it from a borrowed PTK or GTK and moves it into lower-MAC installation;
//! software CCMP and hardware programming borrow the same owner's bytes.
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use core::num::NonZeroU8;

mod key;

pub use key::Ccmp128Key;

pub const CCMP_HEADER_LEN: usize = 8;
pub const CCMP_PACKET_NUMBER_MAX: u64 = (1_u64 << 48) - 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CcmpPacketNumber(u64);

impl CcmpPacketNumber {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Option<Self> {
        if value <= CCMP_PACKET_NUMBER_MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn from_header_bytes(bytes: [u8; CCMP_HEADER_LEN]) -> Self {
        Self(
            (bytes[0] as u64)
                | ((bytes[1] as u64) << 8)
                | ((bytes[4] as u64) << 16)
                | ((bytes[5] as u64) << 24)
                | ((bytes[6] as u64) << 32)
                | ((bytes[7] as u64) << 40),
        )
    }

    /// Decode the little-endian six-octet packet number carried by an EAPOL
    /// Key RSC. The two unused high octets must be zero; accepting them would
    /// silently truncate an install-time replay frontier.
    pub const fn from_receive_sequence(
        sequence: [u8; 8],
    ) -> Result<Self, CcmpReceiveSequenceError> {
        if sequence[6] != 0 || sequence[7] != 0 {
            return Err(CcmpReceiveSequenceError::HighBitsSet);
        }
        Ok(Self(
            (sequence[0] as u64)
                | ((sequence[1] as u64) << 8)
                | ((sequence[2] as u64) << 16)
                | ((sequence[3] as u64) << 24)
                | ((sequence[4] as u64) << 32)
                | ((sequence[5] as u64) << 40),
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpReceiveSequenceError {
    HighBitsSet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CcmpKeyId(u8);

impl CcmpKeyId {
    pub const PAIRWISE: Self = Self(0);

    pub const fn new(value: u8) -> Option<Self> {
        if value <= 3 { Some(Self(value)) } else { None }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    pub const fn header_bits(self) -> u8 {
        self.0 << 6
    }
}

/// The two-bit wire identifier of a GTK, 0 through 3.
///
/// IEEE 802.11-2020, 12.7.2 (EAPOL-Key frames), defines the GTK KDE's
/// two-bit KeyID. A GTK with ID zero does not name the pairwise key:
/// receive selection distinguishes pairwise and group frame addressing.
/// Hardware slot restrictions belong to the backend, separately from this
/// wire value.
///
/// ```
/// # use oer_ieee80211_wire::ccmp::GroupKeyId;
/// assert_eq!(GroupKeyId::new(0).map(GroupKeyId::get), Some(0));
/// assert_eq!(GroupKeyId::new(1).map(GroupKeyId::get), Some(1));
/// assert_eq!(GroupKeyId::new(3).map(GroupKeyId::get), Some(3));
/// assert!(GroupKeyId::new(4).is_none());
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupKeyId(GroupIndex);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GroupIndex {
    Zero,
    One,
    Two,
    Three,
}

impl GroupKeyId {
    /// `key_id`, when it fits the GTK KDE's two-bit field.
    pub const fn new(key_id: u8) -> Option<Self> {
        match key_id {
            0 => Some(Self(GroupIndex::Zero)),
            1 => Some(Self(GroupIndex::One)),
            2 => Some(Self(GroupIndex::Two)),
            3 => Some(Self(GroupIndex::Three)),
            _ => None,
        }
    }

    /// The identifier.
    pub const fn get(self) -> u8 {
        match self.0 {
            GroupIndex::Zero => 0,
            GroupIndex::One => 1,
            GroupIndex::Two => 2,
            GroupIndex::Three => 3,
        }
    }

    /// The identifier, as a CCMP header names it.
    pub const fn key_id(self) -> CcmpKeyId {
        CcmpKeyId(self.get())
    }

    /// Select the record for this wire ID, independently of hardware slots.
    pub const fn select<T>(self, entries: &[T; 4]) -> &T {
        let [zero, one, two, three] = entries;
        match self.0 {
            GroupIndex::Zero => zero,
            GroupIndex::One => one,
            GroupIndex::Two => two,
            GroupIndex::Three => three,
        }
    }

    /// Select the mutable record for this wire ID.
    pub const fn select_mut<T>(self, entries: &mut [T; 4]) -> &mut T {
        let [zero, one, two, three] = entries;
        match self.0 {
            GroupIndex::Zero => zero,
            GroupIndex::One => one,
            GroupIndex::Two => two,
            GroupIndex::Three => three,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CcmpHeader {
    packet_number: CcmpPacketNumber,
    key_id: CcmpKeyId,
}

impl CcmpHeader {
    pub const fn new(packet_number: CcmpPacketNumber, key_id: CcmpKeyId) -> Self {
        Self {
            packet_number,
            key_id,
        }
    }

    pub const fn parse(bytes: [u8; CCMP_HEADER_LEN]) -> Result<Self, CcmpHeaderError> {
        if bytes[2] != 0 {
            return Err(CcmpHeaderError::ReservedOctet);
        }
        if bytes[3] & 0x20 == 0 {
            return Err(CcmpHeaderError::ExtIvMissing);
        }
        if bytes[3] & 0x1f != 0 {
            return Err(CcmpHeaderError::ReservedKeyBits);
        }
        Ok(Self {
            packet_number: CcmpPacketNumber::from_header_bytes(bytes),
            key_id: CcmpKeyId((bytes[3] >> 6) & 0x03),
        })
    }

    pub const fn encode(self) -> [u8; CCMP_HEADER_LEN] {
        let packet_number = self.packet_number.value();
        [
            packet_number as u8,
            (packet_number >> 8) as u8,
            0,
            self.key_id.header_bits() | 0x20,
            (packet_number >> 16) as u8,
            (packet_number >> 24) as u8,
            (packet_number >> 32) as u8,
            (packet_number >> 40) as u8,
        ]
    }

    pub const fn packet_number(self) -> CcmpPacketNumber {
        self.packet_number
    }

    pub const fn key_id(self) -> CcmpKeyId {
        self.key_id
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpHeaderError {
    ReservedOctet,
    ExtIvMissing,
    ReservedKeyBits,
}

/// How far a key's transmit packet number advances per MPDU.
///
/// IEEE Std 802.11-2020 12.5.3.3.2 (PN processing) requires only that the PN of each MPDU
/// under one key increases, and that the fragments of one MSDU take
/// consecutive PNs; one is the ordinary step. A backend or vendor profile
/// that must stay compatible with a peer implementation's numbering supplies
/// its own step: the Espressif `net80211` stack advances by three
/// (`oer-espressif-ieee80211-policy`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CcmpPacketNumberStep(NonZeroU8);

impl CcmpPacketNumberStep {
    /// The ordinary step of one.
    pub const ONE: Self = Self(NonZeroU8::MIN);

    /// A step of `step`; `None` for zero, which would repeat a PN.
    pub const fn new(step: u8) -> Option<Self> {
        match NonZeroU8::new(step) {
            Some(step) => Some(Self(step)),
            None => None,
        }
    }

    pub const fn get(self) -> u8 {
        self.0.get()
    }
}

pub const fn ccmp_header(low: u32, high: u32, key_id_bits: u8) -> [u8; CCMP_HEADER_LEN] {
    [
        low as u8,
        (low >> 8) as u8,
        0,
        key_id_bits | 0x20,
        (low >> 16) as u8,
        (low >> 24) as u8,
        high as u8,
        (high >> 8) as u8,
    ]
}

#[cfg(test)]
mod tests;
