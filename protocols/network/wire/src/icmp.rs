//! ICMPv4 Echo and Echo Reply messages.
//!
//! SOURCE: RFC 792 (Echo or Echo Reply Message). A parse does not check the
//! checksum.

use oer_portable_wire::Reader;

use crate::checksum::Checksum;
use oer_portable_wire::octet_code;

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

octet_code! { "oer_protocols_network_wire::icmp";
    /// The Code of an ICMP message: any octet. What it says is its Type's.
    ///
    /// SOURCE: RFC 792 (Echo or Echo Reply Message, Code).
    IcmpCode {
        /// An echo's and an echo reply's.
        ECHO = 0,
    }
}

oer_portable_wire::bitfield! {
    /// The header of an echo or echo reply.
    /// SOURCE: RFC 792 (Echo or Echo Reply Message).
    struct EchoHeader[8] (big) {
        kind, with_kind: {u8} = 56..64;
        code, with_code: {u8} = 48..56;
        checksum, with_checksum: {u16} = 32..48;
        identifier, with_identifier: {u16} = 16..32;
        sequence, with_sequence: {u16} = 0..16;
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
    pub code: IcmpCode,
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
        let header = EchoHeader::from_bytes(reader.read().map_err(|_| IcmpError::Truncated)?);
        Ok(Self {
            kind: MessageType::from_encoding(header.kind()),
            code: IcmpCode::new(header.code()),
            checksum: header.checksum(),
            identifier: header.identifier(),
            sequence: header.sequence(),
            data: reader.remaining(),
        })
    }

    /// The header of this message with `kind` and its checksum over the
    /// header and the same data, RFC 792 (Checksum).
    pub fn header(&self, kind: MessageType) -> [u8; ECHO_HEADER_LEN] {
        let header = EchoHeader::from_bytes([0; ECHO_HEADER_LEN])
            .with_kind(kind.encoding())
            .with_code(self.code.value())
            .with_identifier(self.identifier)
            .with_sequence(self.sequence);
        let checksum = Checksum::new()
            .with(&header.to_bytes())
            .with(self.data)
            .finish();
        header.with_checksum(checksum).to_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An echo with identifier 17, sequence 7 and data "echo", checksummed.
    fn echo() -> [u8; 12] {
        let request = Echo {
            kind: MessageType::Echo,
            code: IcmpCode::ECHO,
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
