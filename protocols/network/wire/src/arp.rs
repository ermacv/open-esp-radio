//! ARP for IPv4 over Ethernet.
//!
//! SOURCE: RFC 826 (Packet format); hardware and protocol numbers from the
//! IANA ARP Parameters registry.

use oer_portable_wire::{Error, Reader, Writer};

use oer_protocols_ieee802_wire::{EtherType, Eui48};

use crate::address::Ipv4Address;

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

oer_portable_wire::bitfield! {
    /// ar$op: what the packet does.
    /// SOURCE: RFC 826 (Definitions).
    pub enum Operation: u16 {
        /// ares_op$REQUEST.
        Request = 1,
        /// ares_op$REPLY.
        Reply = 2,
    }
}

oer_portable_wire::bitfield! {
    /// The fixed fields, before the addresses.
    /// SOURCE: RFC 826 (Packet format).
    struct Fixed[8] (big) {
        /// ar$hrd.
        hardware, with_hardware: {u16} = 48..64;
        /// ar$pro.
        protocol, with_protocol: {u16} = 32..48;
        /// ar$hln.
        hardware_length, with_hardware_length: {u8} = 24..32;
        /// ar$pln.
        protocol_length, with_protocol_length: {u8} = 16..24;
        /// ar$op.
        operation, with_operation: {u16} = 0..16;
    }
}

/// An ARP packet for IPv4 over Ethernet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Arp {
    pub operation: Operation,
    /// ar$sha.
    pub sender_hardware: Eui48,
    /// ar$spa.
    pub sender_protocol: Ipv4Address,
    /// ar$tha.
    pub target_hardware: Eui48,
    /// ar$tpa.
    pub target_protocol: Ipv4Address,
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
        let fixed = Fixed::from_bytes(reader.read().map_err(|_| ArpError::Truncated)?);
        if fixed.hardware() != HARDWARE_ETHERNET
            || EtherType::new(fixed.protocol()) != EtherType::IPV4
            || fixed.hardware_length() != HARDWARE_LEN
            || fixed.protocol_length() != PROTOCOL_LEN
        {
            return Ok(None);
        }
        let read = |reader: &mut Reader<'_>| -> Result<(Eui48, Ipv4Address), ArpError> {
            let hardware = Eui48::read(reader).map_err(|_| ArpError::Truncated)?;
            let protocol = Ipv4Address::read(reader).map_err(|_| ArpError::Truncated)?;
            Ok((hardware, protocol))
        };
        let (sender_hardware, sender_protocol) = read(&mut reader)?;
        let (target_hardware, target_protocol) = read(&mut reader)?;
        Ok(Some(Self {
            operation: Operation::from_encoding(fixed.operation()),
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
        let fixed = Fixed::from_bytes([0; 8])
            .with_hardware(HARDWARE_ETHERNET)
            .with_protocol(EtherType::IPV4.value())
            .with_hardware_length(HARDWARE_LEN)
            .with_protocol_length(PROTOCOL_LEN)
            .with_operation(self.operation.encoding());
        writer.write(&fixed.to_bytes())?;
        writer.write(self.sender_hardware.as_octets())?;
        writer.write(self.sender_protocol.as_octets())?;
        writer.write(self.target_hardware.as_octets())?;
        writer.write(self.target_protocol.as_octets())
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
        assert_eq!(packet.sender_hardware, Eui48::new([2, 0, 0, 0, 0, 1]));
        assert_eq!(packet.sender_protocol, Ipv4Address::new([192, 0, 2, 1]));
        assert_eq!(packet.target_hardware, Eui48::new([0; 6]));
        assert_eq!(packet.target_protocol, Ipv4Address::new([192, 0, 2, 2]));
        let mut written = [0; PACKET_LEN];
        packet.write(&mut written).unwrap();
        assert_eq!(written, REQUEST);
    }

    #[test]
    fn an_operation_this_codec_does_not_name_survives() {
        let mut bytes = REQUEST;
        bytes[7] = 9;
        let packet = Arp::parse(&bytes).unwrap().unwrap();
        assert_eq!(packet.operation, Operation::Reserved(9));
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
