//! Checked ICMP observations and raw-IP requests using the pinned fork's codecs.
use core::net::Ipv4Addr;
use owned_embassy_net::wire::{Icmpv4Message, Icmpv4Packet, IpProtocol, Ipv4Packet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Invalid {
    Ipv4,
    Fragment,
    Icmp,
    Checksum,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Echo {
    pub source: Ipv4Addr,
    pub destination: Ipv4Addr,
    pub reply: bool,
    pub identifier: u16,
    pub sequence: u16,
}

/// No unchecked wire access precedes complete IPv4 and ICMP validation.
/// Fragmented echoes are refused; W3's bounded echoes fit one Ethernet frame.
pub fn parse(bytes: &mut [u8]) -> Result<Option<Echo>, Invalid> {
    let packet = Ipv4Packet::new_checked(bytes).map_err(|_| Invalid::Ipv4)?;
    if packet.version() != 4 {
        return Err(Invalid::Ipv4);
    }
    if !packet.verify_checksum() {
        return Err(Invalid::Checksum);
    }
    if packet.next_header() != IpProtocol::Icmp {
        return Ok(None);
    }
    if packet.more_frags() || packet.frag_offset() != 0 {
        return Err(Invalid::Fragment);
    }
    let source = packet.src_addr();
    let destination = packet.dst_addr();
    let start = usize::from(packet.header_len());
    let end = usize::from(packet.total_len());
    let payload = bytes.get_mut(start..end).ok_or(Invalid::Ipv4)?;
    let icmp = Icmpv4Packet::new_checked(payload).map_err(|_| Invalid::Icmp)?;
    if !icmp.verify_checksum() {
        return Err(Invalid::Checksum);
    }
    let reply = match (icmp.msg_type(), icmp.msg_code()) {
        (Icmpv4Message::EchoRequest, 0) => false,
        (Icmpv4Message::EchoReply, 0) => true,
        (Icmpv4Message::EchoRequest | Icmpv4Message::EchoReply, _) => return Err(Invalid::Icmp),
        _ => return Ok(None),
    };
    Ok(Some(Echo {
        source,
        destination,
        reply,
        identifier: icmp.echo_ident(),
        sequence: icmp.echo_seq_no(),
    }))
}

pub const REQUEST_LEN: usize = 44;
pub fn request(
    source: Ipv4Addr,
    destination: Ipv4Addr,
    id: u16,
    sequence: u16,
) -> [u8; REQUEST_LEN] {
    let mut bytes = [0; REQUEST_LEN];
    {
        // Fixed local array, exactly 20 bytes of IPv4 header and 24 of ICMP.
        let mut ip = Ipv4Packet::new_unchecked(&mut bytes);
        ip.set_version(4);
        ip.set_header_len(20);
        ip.set_total_len(REQUEST_LEN as u16);
        ip.set_ident(sequence);
        ip.clear_flags();
        ip.set_dont_frag(true);
        ip.set_hop_limit(64);
        ip.set_next_header(IpProtocol::Icmp);
        ip.set_src_addr(source);
        ip.set_dst_addr(destination);
        ip.fill_checksum();
    }
    let mut echo = Icmpv4Packet::new_unchecked(&mut bytes[20..]);
    echo.set_msg_type(Icmpv4Message::EchoRequest);
    echo.set_msg_code(0);
    echo.set_echo_ident(id);
    echo.set_echo_seq_no(sequence);
    echo.fill_checksum();
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_icmp_type_and_code_is_safe_and_echo_requires_code_zero() {
        for kind in 0..=u8::MAX {
            for code in 0..=u8::MAX {
                let mut bytes = request(Ipv4Addr::LOCALHOST, Ipv4Addr::LOCALHOST, 1, 1);
                bytes[20] = kind;
                bytes[21] = code;
                Icmpv4Packet::new_unchecked(&mut bytes[20..]).fill_checksum();
                let parsed = parse(&mut bytes);
                if matches!(kind, 0 | 8) {
                    assert_eq!(parsed.is_ok(), code == 0);
                } else {
                    assert_eq!(parsed, Ok(None));
                }
            }
        }
    }
    #[test]
    fn validates_all_prefixes_checksums_and_echo_identity() {
        let source = Ipv4Addr::new(10, 231, 0, 8);
        let destination = Ipv4Addr::new(10, 231, 0, 1);
        let mut bytes = request(source, destination, 7, 99);
        assert_eq!(
            parse(&mut bytes),
            Ok(Some(Echo {
                source,
                destination,
                reply: false,
                identifier: 7,
                sequence: 99
            }))
        );
        for length in 0..REQUEST_LEN {
            assert!(parse(&mut bytes[..length]).is_err());
        }
        bytes[43] ^= 1;
        assert_eq!(parse(&mut bytes), Err(Invalid::Checksum));
        bytes[43] ^= 1;
        bytes[10] ^= 1;
        assert_eq!(parse(&mut bytes), Err(Invalid::Checksum));
    }
    #[test]
    fn malformed_headers_unknown_types_fragments_and_short_echoes_are_safe() {
        for length in 0..=64 {
            let mut bytes = [0xff; 64];
            let _ = parse(&mut bytes[..length]);
        }
        let mut bytes = request(Ipv4Addr::LOCALHOST, Ipv4Addr::LOCALHOST, 1, 1);
        Ipv4Packet::new_unchecked(&mut bytes).set_more_frags(true);
        Ipv4Packet::new_unchecked(&mut bytes).fill_checksum();
        assert_eq!(parse(&mut bytes), Err(Invalid::Fragment));
        let mut bytes = request(Ipv4Addr::LOCALHOST, Ipv4Addr::LOCALHOST, 1, 1);
        Icmpv4Packet::new_unchecked(&mut bytes[20..]).set_msg_type(Icmpv4Message::EchoReply);
        Icmpv4Packet::new_unchecked(&mut bytes[20..]).fill_checksum();
        assert!(parse(&mut bytes).expect("valid reply").expect("echo").reply);
        bytes[20] = 255;
        Icmpv4Packet::new_unchecked(&mut bytes[20..]).fill_checksum();
        assert_eq!(parse(&mut bytes), Ok(None));
    }
}
