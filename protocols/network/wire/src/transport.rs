//! TCP and UDP header bounds and ports.
//!
//! SOURCE: RFC 9293, 3.1 (Header Format); RFC 768 (User Datagram Protocol);
//! protocol numbers from the IANA Assigned Internet Protocol Numbers
//! registry. Checksums are not checked.

use oer_wire::Reader;

/// Transmission Control Protocol.
pub const PROTOCOL_TCP: u8 = 6;
/// User Datagram Protocol.
pub const PROTOCOL_UDP: u8 = 17;

/// A TCP or UDP header that cannot be one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportError {
    /// Fewer octets than the fixed header.
    Truncated,
    /// TCP's Data Offset or UDP's Length is below the fixed header or past
    /// the octets given.
    Length,
}

oer_wire::bitfield! {
    /// TCP's Data Offset octet.
    /// SOURCE: RFC 9293, 3.1 (Header Format).
    pub struct TcpDataOffset[1] (little) {
        pub reserved, with_reserved: u8 = 0..4;
        pub words, with_words: u8 = 4..8;
    }
}

/// Source and destination port of one TCP or UDP header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ports {
    pub source: u16,
    pub destination: u16,
}

impl Ports {
    /// The four octets of both ports, as they lie in the header.
    pub const fn to_bytes(self) -> [u8; 4] {
        let [source_high, source_low] = self.source.to_be_bytes();
        let [destination_high, destination_low] = self.destination.to_be_bytes();
        [source_high, source_low, destination_high, destination_low]
    }

    /// The ports of the TCP or UDP header at the front of `payload`;
    /// `Ok(None)` for another protocol.
    pub fn parse(protocol: u8, payload: &[u8]) -> Result<Option<Self>, TransportError> {
        let mut reader = Reader::new(payload);
        match protocol {
            PROTOCOL_TCP => {
                let header = reader.read::<20>().map_err(|_| TransportError::Truncated)?;
                let [
                    source_high,
                    source_low,
                    destination_high,
                    destination_low,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    offset,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                ] = header;
                let header_len = usize::from(TcpDataOffset::from_bytes([offset]).words())
                    .checked_mul(4)
                    .ok_or(TransportError::Length)?;
                if header_len < 20 || header_len > payload.len() {
                    return Err(TransportError::Length);
                }
                Ok(Some(Self {
                    source: u16::from_be_bytes([source_high, source_low]),
                    destination: u16::from_be_bytes([destination_high, destination_low]),
                }))
            }
            PROTOCOL_UDP => {
                let header = reader.read::<8>().map_err(|_| TransportError::Truncated)?;
                let [
                    source_high,
                    source_low,
                    destination_high,
                    destination_low,
                    length_high,
                    length_low,
                    _,
                    _,
                ] = header;
                let length = usize::from(u16::from_be_bytes([length_high, length_low]));
                if length < 8 || length > payload.len() {
                    return Err(TransportError::Length);
                }
                Ok(Some(Self {
                    source: u16::from_be_bytes([source_high, source_low]),
                    destination: u16::from_be_bytes([destination_high, destination_low]),
                }))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests;
