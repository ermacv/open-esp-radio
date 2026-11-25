//! The Ethernet II header: destination, source and Type.
//!
//! SOURCE: IEEE Std 802.3-2022, 3.1.1 (MAC frame format) and 3.2.6
//! (Length/Type field); the FCS is not part of these frames.

use oer_portable_wire::{Error, Reader, Writer};
use oer_protocols_ieee802_wire::{EtherType, Eui48};

/// Length of the header, without an FCS.
pub const HEADER_LEN: usize = 14;

/// One Ethernet II header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EthernetHeader {
    pub destination: Eui48,
    pub source: Eui48,
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
            length_type @ ..,
        ] = *bytes;
        Self {
            destination: Eui48::new([d0, d1, d2, d3, d4, d5]),
            source: Eui48::new([s0, s1, s2, s3, s4, s5]),
            ether_type: EtherType::from_octets(length_type),
        }
    }

    /// The fourteen octets of this header.
    pub const fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let [d0, d1, d2, d3, d4, d5] = self.destination.octets();
        let [s0, s1, s2, s3, s4, s5] = self.source.octets();
        let [type_high, type_low] = self.ether_type.octets();
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
