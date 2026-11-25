//! TCP and UDP header bounds and ports.
//!
//! SOURCE: RFC 9293, 3.1 (Header Format); RFC 768 (User Datagram Protocol).
//! Checksums are not checked.

use oer_portable_wire::Reader;

use crate::ip::IpProtocol;

/// A TCP or UDP header that cannot be one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportError {
    /// Fewer octets than the fixed header.
    Truncated,
    /// TCP's Data Offset or UDP's Length is below the fixed header or past
    /// the octets given.
    Length,
}

oer_portable_wire::bitfield! {
    /// TCP's Data Offset octet.
    /// SOURCE: RFC 9293, 3.1 (Header Format).
    pub struct TcpDataOffset[1] (little) {
        pub reserved, with_reserved: u8 = 0..4;
        pub words, with_words: u8 = 4..8;
    }
}

oer_portable_wire::bitfield! {
    /// The ports, which begin a TCP and a UDP header alike.
    /// SOURCE: RFC 9293, 3.1 (Header Format); RFC 768 (Format).
    struct PortFields[4] (big) {
        source, with_source: {u16} = 16..32;
        destination, with_destination: {u16} = 0..16;
    }
}

oer_portable_wire::bitfield! {
    /// UDP's Length and Checksum.
    /// SOURCE: RFC 768 (Format).
    pub(crate) struct UdpNumbers[4] (big) {
        pub(crate) length, with_length: {u16} = 16..32;
        pub(crate) checksum, with_checksum: {u16} = 0..16;
    }
}

/// Length of the UDP header.
pub const UDP_HEADER_LEN: usize = 8;

/// One UDP header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UdpHeader {
    pub ports: Ports,
    /// Header and data, in octets.
    pub length: u16,
    pub checksum: u16,
}

impl UdpHeader {
    /// The header at the front of `datagram` and the data its Length bounds.
    ///
    /// # Errors
    /// [`TransportError`]: fewer octets than the header, or a Length below it
    /// or past the octets given.
    pub fn parse(datagram: &[u8]) -> Result<(Self, &[u8]), TransportError> {
        let mut reader = Reader::new(datagram);
        let ports = reader.read().map_err(|_| TransportError::Truncated)?;
        let numbers = UdpNumbers::from_bytes(reader.read().map_err(|_| TransportError::Truncated)?);
        let length = numbers.length();
        let data = usize::from(length)
            .checked_sub(UDP_HEADER_LEN)
            .and_then(|data| reader.remaining().get(..data))
            .ok_or(TransportError::Length)?;
        Ok((
            Self {
                ports: Ports::from_bytes(ports),
                length,
                checksum: numbers.checksum(),
            },
            data,
        ))
    }

    /// The header's octets.
    pub const fn to_bytes(&self) -> [u8; UDP_HEADER_LEN] {
        let [source_high, source_low, destination_high, destination_low] = self.ports.to_bytes();
        let [length_high, length_low, checksum_high, checksum_low] = UdpNumbers::from_bytes([0; 4])
            .with_length(self.length)
            .with_checksum(self.checksum)
            .to_bytes();
        [
            source_high,
            source_low,
            destination_high,
            destination_low,
            length_high,
            length_low,
            checksum_high,
            checksum_low,
        ]
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
        PortFields::from_bytes([0; 4])
            .with_source(self.source)
            .with_destination(self.destination)
            .to_bytes()
    }

    /// The ports these four octets encode.
    const fn from_bytes(octets: [u8; 4]) -> Self {
        let fields = PortFields::from_bytes(octets);
        Self {
            source: fields.source(),
            destination: fields.destination(),
        }
    }

    /// The ports of the TCP or UDP header at the front of `payload`;
    /// `Ok(None)` for another protocol.
    pub fn parse(protocol: IpProtocol, payload: &[u8]) -> Result<Option<Self>, TransportError> {
        let mut reader = Reader::new(payload);
        match protocol {
            IpProtocol::TCP => {
                let [
                    p0,
                    p1,
                    p2,
                    p3,
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
                ] = reader.read::<20>().map_err(|_| TransportError::Truncated)?;
                let header_len = usize::from(TcpDataOffset::from_bytes([offset]).words())
                    .checked_mul(4)
                    .ok_or(TransportError::Length)?;
                if header_len < 20 || header_len > payload.len() {
                    return Err(TransportError::Length);
                }
                Ok(Some(Self::from_bytes([p0, p1, p2, p3])))
            }
            IpProtocol::UDP => {
                let ports = reader.read().map_err(|_| TransportError::Truncated)?;
                let numbers =
                    UdpNumbers::from_bytes(reader.read().map_err(|_| TransportError::Truncated)?);
                let length = usize::from(numbers.length());
                if length < 8 || length > payload.len() {
                    return Err(TransportError::Length);
                }
                Ok(Some(Self::from_bytes(ports)))
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests;
