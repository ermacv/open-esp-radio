//! ICMPv4 Echo and Echo Reply messages.
//!
//! SOURCE: RFC 792 (Echo or Echo Reply Message); the protocol number from the
//! IANA Assigned Internet Protocol Numbers registry. A parse does not check the
//! checksum.

use oer_portable_wire::Reader;

use crate::checksum::Checksum;

/// Internet Control Message Protocol.
pub const PROTOCOL_ICMP: u8 = 1;
/// RFC 792: Type, Code, Checksum, Identifier and Sequence Number.
pub const ECHO_HEADER_LEN: usize = 8;

oer_portable_wire::bitfield! {
    /// The Type of an ICMP message this codec names.
    /// SOURCE: RFC 792 (Echo or Echo Reply Message, Type).
    pub enum MessageType: u8 {
        EchoReply = 0,
        Echo = 8,
    }
}

/// A message that cannot be an echo.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IcmpError {
    /// Fewer octets than an echo's header.
    Truncated,
}

/// An echo or echo reply over the caller's octets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Echo<'a> {
    pub kind: MessageType,
    /// RFC 792: 0 for an echo.
    pub code: u8,
    pub checksum: u16,
    pub identifier: u16,
    pub sequence: u16,
    /// RFC 792: what an echo reply returns.
    pub data: &'a [u8],
}

impl<'a> Echo<'a> {
    /// The echo header at the front of `message`, and the data after it.
    ///
    /// # Errors
    /// [`IcmpError::Truncated`] below [`ECHO_HEADER_LEN`] octets.
    pub fn parse(message: &'a [u8]) -> Result<Self, IcmpError> {
        let mut reader = Reader::new(message);
        let [kind, code, c0, c1, i0, i1, s0, s1] = reader
            .read::<ECHO_HEADER_LEN>()
            .map_err(|_| IcmpError::Truncated)?;
        Ok(Self {
            kind: MessageType::from_encoding(kind),
            code,
            checksum: u16::from_be_bytes([c0, c1]),
            identifier: u16::from_be_bytes([i0, i1]),
            sequence: u16::from_be_bytes([s0, s1]),
            data: reader.remaining(),
        })
    }

    /// The header of this message with `kind` and its checksum over the
    /// header and the same data, RFC 792 (Checksum).
    pub fn header(&self, kind: MessageType) -> [u8; ECHO_HEADER_LEN] {
        let header = |checksum: u16| {
            let [c0, c1] = checksum.to_be_bytes();
            let [i0, i1] = self.identifier.to_be_bytes();
            let [s0, s1] = self.sequence.to_be_bytes();
            [kind.encoding(), self.code, c0, c1, i0, i1, s0, s1]
        };
        let checksum = Checksum::new().with(&header(0)).with(self.data).finish();
        header(checksum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An echo with identifier 17, sequence 7 and data "echo", checksummed.
    fn echo() -> [u8; 12] {
        let request = Echo {
            kind: MessageType::Echo,
            code: 0,
            checksum: 0,
            identifier: 17,
            sequence: 7,
            data: b"echo",
        };
        let mut message = [0; 12];
        message[..8].copy_from_slice(&request.header(MessageType::Echo));
        message[8..].copy_from_slice(b"echo");
        message
    }

    #[test]
    fn an_echo_reads_its_fields_and_its_checksum_verifies() {
        let message = echo();
        assert_eq!(&message[..2], &[8, 0]);
        assert!(Checksum::new().with(&message).verifies());
        let parsed = Echo::parse(&message).unwrap();
        assert_eq!(parsed.kind, MessageType::Echo);
        assert_eq!((parsed.identifier, parsed.sequence), (17, 7));
        assert_eq!(parsed.data, b"echo");
    }

    #[test]
    fn a_reply_header_keeps_identity_and_data_under_a_new_checksum() {
        let message = echo();
        let parsed = Echo::parse(&message).unwrap();
        let header = parsed.header(MessageType::EchoReply);
        assert_eq!(header[0], 0);
        assert_eq!(&header[4..], &message[4..8]);
        assert!(Checksum::new().with(&header).with(parsed.data).verifies());
    }

    #[test]
    fn every_short_message_is_truncated() {
        let message = echo();
        for length in 0..ECHO_HEADER_LEN {
            assert_eq!(Echo::parse(&message[..length]), Err(IcmpError::Truncated));
        }
        assert_eq!(Echo::parse(&message[..8]).unwrap().data, b"");
    }
}
