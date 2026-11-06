//! ARP for IPv4 over Ethernet.
//!
//! SOURCE: RFC 826 (Packet format); hardware and protocol numbers from the
//! IANA ARP Parameters registry.

use oer_portable_wire::{Error, Reader, Writer};

use crate::ethernet::EtherType;

/// Length of an ARP packet for IPv4 over Ethernet.
pub const PACKET_LEN: usize = 28;
/// ar$hrd: Ethernet.
const HARDWARE_ETHERNET: u16 = 1;
/// ar$hln: a MAC address.
const HARDWARE_LEN: u8 = 6;
/// ar$pln: an IPv4 address.
const PROTOCOL_LEN: u8 = 4;

/// A packet that cannot be ARP.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArpError {
    /// Fewer octets than the packet's fixed fields.
    Truncated,
}

/// ar$op: what the packet does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    /// ares_op$REQUEST.
    Request,
    /// ares_op$REPLY.
    Reply,
    /// Another registered operation, which this codec does not interpret.
    Other(u16),
}

impl Operation {
    const fn from_number(number: u16) -> Self {
        match number {
            1 => Self::Request,
            2 => Self::Reply,
            other => Self::Other(other),
        }
    }

    const fn number(self) -> u16 {
        match self {
            Self::Request => 1,
            Self::Reply => 2,
            Self::Other(other) => other,
        }
    }
}

/// An ARP packet for IPv4 over Ethernet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Arp {
    pub operation: Operation,
    /// ar$sha.
    pub sender_hardware: [u8; 6],
    /// ar$spa.
    pub sender_protocol: [u8; 4],
    /// ar$tha.
    pub target_hardware: [u8; 6],
    /// ar$tpa.
    pub target_protocol: [u8; 4],
}

impl Arp {
    /// The packet at the front of `payload`; `Ok(None)` for ARP of another
    /// hardware or protocol, which this codec does not interpret.
    ///
    /// # Errors
    /// [`ArpError::Truncated`] when the eight fixed octets, or the addresses
    /// they announce, do not fit.
    pub fn parse(payload: &[u8]) -> Result<Option<Self>, ArpError> {
        let mut reader = Reader::new(payload);
        let fixed = reader.array::<8>().map_err(|_| ArpError::Truncated)?;
        let [
            hardware_high,
            hardware_low,
            protocol_high,
            protocol_low,
            hardware_len,
            protocol_len,
            operation_high,
            operation_low,
        ] = *fixed;
        if u16::from_be_bytes([hardware_high, hardware_low]) != HARDWARE_ETHERNET
            || EtherType::new(u16::from_be_bytes([protocol_high, protocol_low])) != EtherType::IPV4
            || hardware_len != HARDWARE_LEN
            || protocol_len != PROTOCOL_LEN
        {
            return Ok(None);
        }
        let read = |reader: &mut Reader<'_>| -> Result<([u8; 6], [u8; 4]), ArpError> {
            let hardware = *reader.array().map_err(|_| ArpError::Truncated)?;
            let protocol = *reader.array().map_err(|_| ArpError::Truncated)?;
            Ok((hardware, protocol))
        };
        let (sender_hardware, sender_protocol) = read(&mut reader)?;
        let (target_hardware, target_protocol) = read(&mut reader)?;
        Ok(Some(Self {
            operation: Operation::from_number(u16::from_be_bytes([operation_high, operation_low])),
            sender_hardware,
            sender_protocol,
            target_hardware,
            target_protocol,
        }))
    }

    /// Write the packet at the front of `buffer`.
    ///
    /// # Errors
    /// [`Error`] when `buffer` is shorter than [`PACKET_LEN`].
    pub fn write(&self, buffer: &mut [u8]) -> Result<(), Error> {
        let mut writer = Writer::new(buffer);
        writer.u16_be(HARDWARE_ETHERNET)?;
        writer.u16_be(EtherType::IPV4.value())?;
        writer.write(&[HARDWARE_LEN, PROTOCOL_LEN])?;
        writer.u16_be(self.operation.number())?;
        writer.write(&self.sender_hardware)?;
        writer.write(&self.sender_protocol)?;
        writer.write(&self.target_hardware)?;
        writer.write(&self.target_protocol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Who has 192.0.2.2? Tell 192.0.2.1: the octets a capture shows.
    const REQUEST: [u8; PACKET_LEN] = [
        0, 1, 8, 0, 6, 4, 0, 1, 2, 0, 0, 0, 0, 1, 192, 0, 2, 1, 0, 0, 0, 0, 0, 0, 192, 0, 2, 2,
    ];

    #[test]
    fn a_request_reads_and_writes_as_it_lies_on_the_wire() {
        let packet = Arp::parse(&REQUEST).unwrap().unwrap();
        assert_eq!(packet.operation, Operation::Request);
        assert_eq!(packet.sender_hardware, [2, 0, 0, 0, 0, 1]);
        assert_eq!(packet.sender_protocol, [192, 0, 2, 1]);
        assert_eq!(packet.target_hardware, [0; 6]);
        assert_eq!(packet.target_protocol, [192, 0, 2, 2]);
        let mut written = [0; PACKET_LEN];
        packet.write(&mut written).unwrap();
        assert_eq!(written, REQUEST);
    }

    #[test]
    fn an_operation_this_codec_does_not_name_survives() {
        let mut bytes = REQUEST;
        bytes[7] = 9;
        let packet = Arp::parse(&bytes).unwrap().unwrap();
        assert_eq!(packet.operation, Operation::Other(9));
        let mut written = [0; PACKET_LEN];
        packet.write(&mut written).unwrap();
        assert_eq!(written, bytes);
    }

    #[test]
    fn arp_of_another_kind_is_no_error() {
        for (index, value) in [(1, 6), (3, 0xdd), (4, 8), (5, 16)] {
            let mut bytes = REQUEST;
            bytes[index] = value;
            assert_eq!(Arp::parse(&bytes), Ok(None), "octet {index}");
        }
    }

    #[test]
    fn every_short_packet_is_truncated_never_a_panic() {
        for length in 0..PACKET_LEN {
            assert_eq!(Arp::parse(&REQUEST[..length]), Err(ArpError::Truncated));
        }
    }

    #[test]
    fn a_short_buffer_refuses_the_write() {
        let packet = Arp::parse(&REQUEST).unwrap().unwrap();
        assert!(packet.write(&mut [0; PACKET_LEN - 1]).is_err());
    }
}
