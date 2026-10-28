//! Passive traffic classification. These observations grant no RX admission.
use super::{EthernetFrameParts, decapsulate_payload};
use crate::frame::DecodedHeader;

/// One mutually exclusive class, including input that cannot be identified.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Traffic {
    EchoRequest,
    EchoReply,
    OtherIcmp,
    OtherIpv4,
    Arp,
    Ipv6,
    Other,
    Unidentified,
}

oer_wire::bitfield! {
    /// SOURCE: RFC 791 section 3.1, Flags and Fragment Offset.
    struct Fragment[2] (big) {
        offset, with_offset: u16 = 0..13;
        more, with_more: u8 = 13..14;
    }
}

impl Traffic {
    /// Classify a bounded Ethernet payload without modifying or admitting it.
    pub fn ethernet(parts: EthernetFrameParts<'_>) -> Self {
        match parts.ether_type {
            0x0806 => Self::Arp,
            0x86dd => Self::Ipv6,
            0x0800 => Self::ipv4(parts.payload),
            _ => Self::Other,
        }
    }

    fn ipv4(bytes: &[u8]) -> Self {
        let Some(header) = bytes.first_chunk::<20>() else {
            return Self::Unidentified;
        };
        let version = crate::qos::Ipv4VersionIhl::from_bytes([header[0]]);
        let length = usize::from(version.ihl()).saturating_mul(4);
        let total = usize::from(u16::from_be_bytes([header[2], header[3]]));
        if version.version() != 4 || length < 20 || total < length || total > bytes.len() {
            return Self::Unidentified;
        }
        if header[9] != 1 {
            return Self::OtherIpv4;
        }
        let fragment = Fragment::from_bytes([header[6], header[7]]);
        if fragment.offset() != 0 || fragment.more() != 0 {
            return Self::Unidentified;
        }
        let Some(icmp) = bytes.get(length..total).and_then(|b| b.first_chunk::<8>()) else {
            return Self::Unidentified;
        };
        // SOURCE: RFC 792, Echo or Echo Reply Message (Type and Code).
        match (icmp[0], icmp[1]) {
            (8, 0) => Self::EchoRequest,
            (0, 0) => Self::EchoReply,
            _ => Self::OtherIcmp,
        }
    }

    /// A single unprotected MSDU. Aggregates/encrypted input stay explicit;
    /// their bytes are never guessed to be an IP packet.
    pub fn mpdu(bytes: &[u8]) -> Self {
        let Some(header) = DecodedHeader::decode(bytes) else {
            return Self::Unidentified;
        };
        if !header.control().is_data() || !matches!(header.control().subtype(), 0..=3 | 8..=11) {
            return Self::Other;
        }
        if header.control().protected() {
            return Self::Unidentified;
        }
        let Ok(mut frames) = decapsulate_payload(header, header.body()) else {
            return Self::Unidentified;
        };
        let Some(Ok(parts)) = frames.next() else {
            return Self::Unidentified;
        };
        if frames.next().is_some() {
            return Self::Unidentified;
        }
        Self::ethernet(parts)
    }
}

/// Saturating, single-owner counts. The sum includes unidentified input.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TrafficCounts {
    pub echo_request: u32,
    pub echo_reply: u32,
    pub other_icmp: u32,
    pub other_ipv4: u32,
    pub arp: u32,
    pub ipv6: u32,
    pub other: u32,
    pub unidentified: u32,
}
impl TrafficCounts {
    pub fn record(&mut self, traffic: Traffic) {
        let count = match traffic {
            Traffic::EchoRequest => &mut self.echo_request,
            Traffic::EchoReply => &mut self.echo_reply,
            Traffic::OtherIcmp => &mut self.other_icmp,
            Traffic::OtherIpv4 => &mut self.other_ipv4,
            Traffic::Arp => &mut self.arp,
            Traffic::Ipv6 => &mut self.ipv6,
            Traffic::Other => &mut self.other,
            Traffic::Unidentified => &mut self.unidentified,
        };
        *count = count.saturating_add(1);
    }
    pub fn total(self) -> u32 {
        [
            self.echo_request,
            self.echo_reply,
            self.other_icmp,
            self.other_ipv4,
            self.arp,
            self.ipv6,
            self.other,
            self.unidentified,
        ]
        .into_iter()
        .fold(0u32, u32::saturating_add)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classification_is_bounded_and_does_not_admit_fragmented_or_truncated_echoes() {
        let mut ip = [0; 28];
        ip[0] = 0x45;
        ip[2..4].copy_from_slice(&28u16.to_be_bytes());
        ip[9] = 1;
        ip[20] = 8;
        assert_eq!(Traffic::ipv4(&ip), Traffic::EchoRequest);
        for len in 0..28 {
            assert_eq!(Traffic::ipv4(&ip[..len]), Traffic::Unidentified);
        }
        ip[20] = 0;
        assert_eq!(Traffic::ipv4(&ip), Traffic::EchoReply);
        ip[20] = 3;
        assert_eq!(Traffic::ipv4(&ip), Traffic::OtherIcmp);
        ip[6] = 0x20;
        assert_eq!(Traffic::ipv4(&ip), Traffic::Unidentified);
        ip[6] = 0;
        ip[9] = 17;
        assert_eq!(Traffic::ipv4(&ip), Traffic::OtherIpv4);
        let mut counts = TrafficCounts {
            echo_request: u32::MAX,
            ..Default::default()
        };
        counts.record(Traffic::EchoRequest);
        counts.record(Traffic::Arp);
        assert_eq!(
            (counts.echo_request, counts.arp, counts.total()),
            (u32::MAX, 1, u32::MAX)
        );
    }
}
