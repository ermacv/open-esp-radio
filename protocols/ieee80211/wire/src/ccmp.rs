//! CCMP packet-number values and eight-byte header codecs.
//!
//! Packet-number allocation and receive replay frontiers belong to the
//! upper-MAC core. This module validates values and encodes or parses their
//! wire representation without retaining an exchange's mutable state.
//!
//! The temporal key is not a format: its owner is
//! `oer-ieee80211-contracts-security`.
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

pub mod protected;

pub const CCMP_HEADER_LEN: usize = 8;
/// The MIC a CCMP-128 MPDU carries after its payload.
///
/// SOURCE: IEEE Std 802.11-2020, 12.5.3.2 (CCMP MPDU format: an 8-octet MIC).
pub const CCMP_MIC_LEN: usize = 8;
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
        Self(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[4], bytes[5], bytes[6], bytes[7], 0, 0,
        ]))
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
        Ok(Self(u64::from_le_bytes(sequence)))
    }
}

/// An ESP-NOW packet number is a nonzero CCMP packet number.
impl From<crate::extensions::espressif::esp_now::EspNowCcmpPacketNumber> for CcmpPacketNumber {
    fn from(packet_number: crate::extensions::espressif::esp_now::EspNowCcmpPacketNumber) -> Self {
        Self(packet_number.value())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpReceiveSequenceError {
    HighBitsSet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CcmpKeyId(oer_wire::U8<2>);

impl CcmpKeyId {
    pub const PAIRWISE: Self = Self(oer_wire::U8::ZERO);

    pub const fn new(value: u8) -> Option<Self> {
        match oer_wire::U8::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn value(self) -> u8 {
        self.0.get()
    }

    pub const fn header_bits(self) -> u8 {
        CcmpKeyControl::from_bytes([0])
            .with_key_id(self.0)
            .to_bytes()[0]
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
        CcmpKeyId(match self.0 {
            GroupIndex::Zero => oer_wire::U8::constant::<0>(),
            GroupIndex::One => oer_wire::U8::constant::<1>(),
            GroupIndex::Two => oer_wire::U8::constant::<2>(),
            GroupIndex::Three => oer_wire::U8::constant::<3>(),
        })
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
        if !CcmpKeyControl::from_bytes([bytes[3]]).extended_iv() {
            return Err(CcmpHeaderError::ExtIvMissing);
        }
        if CcmpKeyControl::from_bytes([bytes[3]]).reserved() != 0 {
            return Err(CcmpHeaderError::ReservedKeyBits);
        }
        Ok(Self {
            packet_number: CcmpPacketNumber::from_header_bytes(bytes),
            key_id: CcmpKeyId(CcmpKeyControl::from_bytes([bytes[3]]).key_id()),
        })
    }

    pub const fn encode(self) -> [u8; CCMP_HEADER_LEN] {
        let [p0, p1, p2, p3, p4, p5, _, _] = self.packet_number.value().to_le_bytes();
        let control = CcmpKeyControl::from_bytes([0])
            .with_extended_iv(true)
            .with_key_id(self.key_id.0);
        [p0, p1, 0, control.to_bytes()[0], p2, p3, p4, p5]
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

pub const fn ccmp_header(low: u32, high: u32, key_id_bits: u8) -> [u8; CCMP_HEADER_LEN] {
    let [p0, p1, p2, p3] = low.to_le_bytes();
    let [p4, p5, _, _] = high.to_le_bytes();
    [
        p0,
        p1,
        0,
        CcmpKeyControl::from_bytes([key_id_bits])
            .with_extended_iv(true)
            .to_bytes()[0],
        p2,
        p3,
        p4,
        p5,
    ]
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 12.5.3.2 (CCMP MPDU format).
    pub struct CcmpKeyControl[1] (little) {
        pub reserved, with_reserved: u8 = 0..5;
        pub extended_iv, with_extended_iv: bool = 5..6;
        pub key_id, with_key_id: (u8) = 6..8;
    }
}

#[cfg(test)]
mod tests;
