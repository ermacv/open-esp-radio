//! The Ethernet II header: destination, source and Type.
//!
//! SOURCE: IEEE Std 802.3-2022, 3.1.1 (MAC frame format) and 3.2.6
//! (Length/Type field); the FCS is not part of these frames.

use oer_wire::{Error, Reader, Writer};

/// Length of the header, without an FCS.
pub const HEADER_LEN: usize = 14;

/// An EtherType: the Type interpretation of the Length/Type field, as the
/// big-endian number of its registry entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct EtherType(u16);

impl EtherType {
    /// Internet Protocol version 4.
    ///
    /// SOURCE: IEEE Registration Authority EtherType registry; RFC 894.
    pub const IPV4: Self = Self(0x0800);
    /// Address Resolution Protocol.
    ///
    /// SOURCE: IEEE Registration Authority EtherType registry; RFC 826.
    pub const ARP: Self = Self(0x0806);
    /// Customer VLAN tag (C-TAG).
    ///
    /// SOURCE: IEEE Std 802.1Q-2022, 9.5 (Tag Protocol Identification), Table 9-1.
    pub const VLAN: Self = Self(0x8100);
    /// Internet Protocol version 6.
    ///
    /// SOURCE: IEEE Registration Authority EtherType registry; RFC 8200.
    pub const IPV6: Self = Self(0x86dd);
    /// Service VLAN tag (S-TAG).
    ///
    /// SOURCE: IEEE Std 802.1Q-2022, 9.5 (Tag Protocol Identification), Table 9-1.
    pub const PROVIDER_VLAN: Self = Self(0x88a8);

    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u16 {
        self.0
    }

    /// Whether the field names a type rather than an IEEE 802.3 length.
    ///
    /// SOURCE: IEEE Std 802.3-2022, 3.2.6: values of 1536 (0x0600) and above
    /// are types.
    pub const fn is_type(self) -> bool {
        self.0 >= 0x0600
    }

    /// Whether the type names a VLAN tag (C-TAG or S-TAG).
    pub const fn is_vlan_tag(self) -> bool {
        matches!(self, Self::VLAN | Self::PROVIDER_VLAN)
    }
}

/// One Ethernet II header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EthernetHeader {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ether_type: EtherType,
}

impl EthernetHeader {
    /// The header at the front of `frame` and the payload after it.
    pub fn parse(frame: &[u8]) -> Result<(Self, &[u8]), Error> {
        let mut reader = Reader::new(frame);
        let header = Self::from_bytes(reader.array::<HEADER_LEN>()?);
        Ok((header, reader.remaining()))
    }

    /// The header these fourteen octets encode.
    pub const fn from_bytes(bytes: &[u8; HEADER_LEN]) -> Self {
        let [
            d0,
            d1,
            d2,
            d3,
            d4,
            d5,
            s0,
            s1,
            s2,
            s3,
            s4,
            s5,
            type_high,
            type_low,
        ] = *bytes;
        Self {
            destination: [d0, d1, d2, d3, d4, d5],
            source: [s0, s1, s2, s3, s4, s5],
            ether_type: EtherType(u16::from_be_bytes([type_high, type_low])),
        }
    }

    /// The fourteen octets of this header.
    pub const fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let [d0, d1, d2, d3, d4, d5] = self.destination;
        let [s0, s1, s2, s3, s4, s5] = self.source;
        let [type_high, type_low] = self.ether_type.0.to_be_bytes();
        [
            d0, d1, d2, d3, d4, d5, s0, s1, s2, s3, s4, s5, type_high, type_low,
        ]
    }

    /// Write the header at the writer's position; a refusal writes nothing.
    pub fn write(&self, writer: &mut Writer<'_>) -> Result<(), Error> {
        writer.write(&self.to_bytes())
    }
}

#[cfg(test)]
mod tests;
