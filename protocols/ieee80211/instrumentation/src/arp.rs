//! ARP correlation of the protected link: the IPv4 addresses of an ARP MSDU
//! and the public header identity (sequence number, CCMP packet number and
//! key ID) of the MPDU that carried it, so a collector can match an ARP
//! exchange against encrypted air traffic without any traffic key.
//!
//! One ARP MSDU is two events, emitted back to back by the same owner:
//! [`ArpAddresses`] first, then [`ArpTransmitted`] or [`ArpReceived`].

use oer_trace::{Domain, Event, Kind};

use crate::ARP;
use crate::event::coded;

/// ARP hardware type of Ethernet.
///
/// SOURCE: RFC 826 (ar$hrd); IANA ARP Parameters, Hardware Types, 1.
const HARDWARE_ETHERNET: u16 = 1;
/// ARP protocol type of IPv4: its EtherType.
///
/// SOURCE: RFC 826 (ar$pro); IEEE Registration Authority EtherType registry.
const PROTOCOL_IPV4: u16 = 0x0800;
/// Ethernet hardware address length.
///
/// SOURCE: RFC 826 (ar$hln).
const HARDWARE_LENGTH: u8 = 6;
/// IPv4 protocol address length.
///
/// SOURCE: RFC 826 (ar$pln).
const PROTOCOL_LENGTH: u8 = 4;

/// The sequence-number space of an MPDU: 12 bits.
///
/// SOURCE: IEEE Std 802.11-2020, 9.2.4.4.1 (Sequence Number field).
const SEQUENCE_SPACE: u16 = 4096;

coded!(
    /// The ARP operation.
    ///
    /// SOURCE: RFC 826 (ar$op): 1 request, 2 reply.
    pub enum ArpOperation {
        Request = 1,
        Reply = 2,
    }
);

/// The IPv4 sender and target protocol addresses of one ARP MSDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArpAddresses {
    pub sender: [u8; 4],
    pub target: [u8; 4],
}

impl ArpAddresses {
    /// The operation and addresses of an Ethernet/IPv4 ARP request or reply
    /// at the start of `payload`; `None` for another hardware or protocol
    /// type, address length or operation, or a payload too short.
    ///
    /// SOURCE: RFC 826, packet format: ar$hrd (2), ar$pro (2), ar$hln (1),
    /// ar$pln (1), ar$op (2), ar$sha (hln), ar$spa (pln), ar$tha (hln),
    /// ar$tpa (pln), all big-endian.
    pub fn parse(payload: &[u8]) -> Option<(ArpOperation, Self)> {
        let (hardware, rest) = payload.split_first_chunk::<2>()?;
        let (protocol, rest) = rest.split_first_chunk::<2>()?;
        let ([hardware_length, protocol_length], rest) = rest.split_first_chunk::<2>()?;
        let (operation, rest) = rest.split_first_chunk::<2>()?;
        let (_sender_hardware, rest) = rest.split_first_chunk::<6>()?;
        let (sender, rest) = rest.split_first_chunk::<4>()?;
        let (_target_hardware, rest) = rest.split_first_chunk::<6>()?;
        let (target, _) = rest.split_first_chunk::<4>()?;
        if u16::from_be_bytes(*hardware) != HARDWARE_ETHERNET
            || u16::from_be_bytes(*protocol) != PROTOCOL_IPV4
            || *hardware_length != HARDWARE_LENGTH
            || *protocol_length != PROTOCOL_LENGTH
        {
            return None;
        }
        let operation = ArpOperation::of_code(u32::from(u16::from_be_bytes(*operation)))?;
        Some((
            operation,
            Self {
                sender: *sender,
                target: *target,
            },
        ))
    }
}

impl Event for ArpAddresses {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 32);
    const CHANNEL: oer_trace::Channel = ARP;

    fn encode(&self) -> [u32; 2] {
        [
            u32::from_be_bytes(self.sender),
            u32::from_be_bytes(self.target),
        ]
    }

    fn decode([sender, target]: [u32; 2]) -> Option<Self> {
        Some(Self {
            sender: sender.to_be_bytes(),
            target: target.to_be_bytes(),
        })
    }
}

impl core::fmt::Display for ArpAddresses {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let [a, b, c, d] = self.sender;
        let [e, g, h, i] = self.target;
        write!(f, "arp.addresses {a}.{b}.{c}.{d} -> {e}.{g}.{h}.{i}")
    }
}

/// The two words of an MPDU's public identity: the 12-bit sequence number
/// with a 4-bit `flags` value above it and the 48-bit packet number.
fn encode_identity(flags: u16, sequence: u16, packet_number: u64) -> [u32; 2] {
    // Both operands are below their fields' bounds, so neither wraps.
    let packed = flags
        .wrapping_mul(SEQUENCE_SPACE)
        .wrapping_add(sequence.checked_rem(SEQUENCE_SPACE).unwrap_or(0));
    let [low, high] = packed.to_le_bytes();
    let [pn0, pn1, pn2, pn3, pn4, pn5, _, _] = packet_number.to_le_bytes();
    [
        u32::from_le_bytes([low, high, pn4, pn5]),
        u32::from_le_bytes([pn0, pn1, pn2, pn3]),
    ]
}

/// `(flags, sequence, packet_number)` of [`encode_identity`]'s words.
fn decode_identity([first, second]: [u32; 2]) -> Option<(u16, u16, u64)> {
    let [low, high, pn4, pn5] = first.to_le_bytes();
    let [pn0, pn1, pn2, pn3] = second.to_le_bytes();
    let packed = u16::from_le_bytes([low, high]);
    Some((
        packed.checked_div(SEQUENCE_SPACE)?,
        packed.checked_rem(SEQUENCE_SPACE)?,
        u64::from_le_bytes([pn0, pn1, pn2, pn3, pn4, pn5, 0, 0]),
    ))
}

/// The station transmitted the ARP MSDU of the [`ArpAddresses`] before.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArpTransmitted {
    pub operation: ArpOperation,
    /// The MPDU's sequence number, 0..4096.
    pub sequence: u16,
    /// The CCMP packet number allocated to it, 48 bits.
    pub packet_number: u64,
}

impl Event for ArpTransmitted {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 33);
    const CHANNEL: oer_trace::Channel = ARP;

    fn encode(&self) -> [u32; 2] {
        let flags = match self.operation {
            ArpOperation::Request => 0,
            ArpOperation::Reply => 1,
        };
        encode_identity(flags, self.sequence, self.packet_number)
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        let (flags, sequence, packet_number) = decode_identity(words)?;
        let operation = match flags {
            0 => ArpOperation::Request,
            1 => ArpOperation::Reply,
            _ => return None,
        };
        Some(Self {
            operation,
            sequence,
            packet_number,
        })
    }
}

impl core::fmt::Display for ArpTransmitted {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "arp.transmitted {:?} sn={} pn={}",
            self.operation, self.sequence, self.packet_number
        )
    }
}

/// The station received the ARP MSDU of the [`ArpAddresses`] before, in a
/// protected MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArpReceived {
    pub operation: ArpOperation,
    /// Whether its destination was the broadcast address.
    pub group_addressed: bool,
    /// The CCMP key ID, 0..=3.
    pub key_id: u8,
    /// The MPDU's sequence number, 0..4096.
    pub sequence: u16,
    /// The MPDU's CCMP packet number, 48 bits.
    pub packet_number: u64,
}

impl Event for ArpReceived {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 34);
    const CHANNEL: oer_trace::Channel = ARP;

    fn encode(&self) -> [u32; 2] {
        // flags = operation (1 bit), group addressing (1 bit), key ID (2 bits)
        let operation = match self.operation {
            ArpOperation::Request => 0,
            ArpOperation::Reply => 1,
        };
        let flags = u16::from(self.key_id.checked_rem(4).unwrap_or(0))
            .wrapping_mul(4)
            .wrapping_add(u16::from(self.group_addressed).wrapping_mul(2))
            .wrapping_add(operation);
        encode_identity(flags, self.sequence, self.packet_number)
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        let (flags, sequence, packet_number) = decode_identity(words)?;
        let operation = match flags.checked_rem(2)? {
            0 => ArpOperation::Request,
            _ => ArpOperation::Reply,
        };
        let group_addressed = flags.checked_div(2)?.checked_rem(2)? == 1;
        let key_id = u8::try_from(flags.checked_div(4)?).ok()?;
        Some(Self {
            operation,
            group_addressed,
            key_id,
            sequence,
            packet_number,
        })
    }
}

impl core::fmt::Display for ArpReceived {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "arp.received {:?} group={} key_id={} sn={} pn={}",
            self.operation, self.group_addressed, self.key_id, self.sequence, self.packet_number
        )
    }
}
