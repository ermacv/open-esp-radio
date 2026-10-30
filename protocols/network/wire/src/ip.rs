//! IPv4 and IPv6 header fields.
//!
//! SOURCE: RFC 791, 3.1 (Internet Header Format); RFC 8200, 3 (IPv6 Header
//! Format); RFC 2474, 3 (Differentiated Services Field). Options and
//! extension headers are not interpreted; checksums are not checked.

use oer_wire::Reader;

const IPV4_MIN_LEN: usize = 20;
const IPV6_LEN: usize = 40;

/// A header that cannot be the version it claims.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IpError {
    /// Fewer octets than the fixed header.
    Truncated,
    /// The Version field names another version.
    Version,
    /// IPv4's Internet Header Length is below five words or past the total
    /// length.
    HeaderLength,
    /// The total (IPv4) or payload (IPv6) length runs past the octets given.
    Length,
}

/// Valid six-bit IPv4/IPv6 Differentiated Services Code Point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dscp(oer_wire::U8<6>);

impl Dscp {
    pub const fn new(value: u8) -> Option<Self> {
        match oer_wire::U8::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn value(self) -> u8 {
        self.0.get()
    }

    /// The codepoint a Differentiated Services field carries.
    pub const fn from_field(value: oer_wire::U8<6>) -> Self {
        Self(value)
    }

    /// The three most significant bits: the Class Selector codepoint.
    ///
    /// SOURCE: RFC 2474, 4.2.2 (Class Selector Codepoints).
    pub const fn class_selector(self) -> u8 {
        DsField::from_bytes([0]).with_dscp(self.0).class_selector()
    }
}

oer_wire::bitfield! {
    /// IPv4's first octet: version and Internet Header Length.
    /// SOURCE: RFC 791, 3.1 (Internet Header Format).
    pub struct Ipv4VersionIhl[1] (little) {
        pub ihl, with_ihl: u8 = 0..4;
        pub version, with_version: u8 = 4..8;
    }
}

oer_wire::bitfield! {
    /// Differentiated Services field, also carried by IPv6 Traffic Class.
    /// SOURCE: RFC 2474, 3 (Differentiated Services Field).
    pub struct DsField[1] (little) {
        /// SOURCE: RFC 3168, 5 (Explicit Congestion Notification in IP).
        pub ecn, with_ecn: u8 = 0..2;
        pub dscp, with_dscp: (u8) = 2..8;
        /// Three DSCP MSBs used by the default priority mapping.
        /// SOURCE: RFC 2474, 4.2.2 (Class Selector Codepoints).
        pub class_selector, with_class_selector: u8 = 5..8, view;
    }
}

oer_wire::bitfield! {
    /// IPv4 Flags and Fragment Offset.
    /// SOURCE: RFC 791, 3.1 (Internet Header Format).
    pub struct Ipv4Fragment[2] (big) {
        pub offset, with_offset: u16 = 0..13;
        pub more_fragments, with_more_fragments: bool = 13..14;
        pub dont_fragment, with_dont_fragment: bool = 14..15;
        pub reserved, with_reserved: bool = 15..16;
    }
}

oer_wire::bitfield! {
    /// First two IPv6 header octets, including the complete Traffic Class.
    /// SOURCE: RFC 8200, 3 (IPv6 Header Format).
    pub struct Ipv6HeaderPrefix[2] (big) {
        pub flow_label_high, with_flow_label_high: u8 = 0..4;
        pub traffic_class, with_traffic_class: u8 = 4..12;
        pub version, with_version: u8 = 12..16;
    }
}

/// A checked IPv4 header over the caller's packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ipv4Header<'a> {
    services: DsField,
    fragment: Ipv4Fragment,
    protocol: u8,
    addresses: [u8; 8],
    payload: &'a [u8],
}

impl<'a> Ipv4Header<'a> {
    /// Length of the header without options.
    pub const MIN_LEN: usize = IPV4_MIN_LEN;

    /// Check the header at the front of `packet`: version 4, a header length
    /// of at least five words within a total length within `packet`.
    pub fn parse(packet: &'a [u8]) -> Result<Self, IpError> {
        let mut reader = Reader::new(packet);
        let fixed = reader
            .read::<IPV4_MIN_LEN>()
            .map_err(|_| IpError::Truncated)?;
        let [
            version_ihl,
            services,
            length_high,
            length_low,
            _,
            _,
            fragment_high,
            fragment_low,
            _,
            protocol,
            _,
            _,
            addresses @ ..,
        ] = fixed;
        let version_ihl = Ipv4VersionIhl::from_bytes([version_ihl]);
        if version_ihl.version() != 4 {
            return Err(IpError::Version);
        }
        let header_len = usize::from(version_ihl.ihl())
            .checked_mul(4)
            .ok_or(IpError::HeaderLength)?;
        let total_len = usize::from(u16::from_be_bytes([length_high, length_low]));
        if header_len < Self::MIN_LEN || header_len > total_len {
            return Err(IpError::HeaderLength);
        }
        let payload = packet
            .get(..total_len)
            .ok_or(IpError::Length)?
            .get(header_len..)
            .ok_or(IpError::HeaderLength)?;
        Ok(Self {
            services: DsField::from_bytes([services]),
            fragment: Ipv4Fragment::from_bytes([fragment_high, fragment_low]),
            protocol,
            addresses,
            payload,
        })
    }

    pub const fn services(&self) -> DsField {
        self.services
    }

    pub const fn dscp(&self) -> Dscp {
        Dscp::from_field(self.services.dscp())
    }

    pub const fn protocol(&self) -> u8 {
        self.protocol
    }

    /// Source then destination address.
    pub const fn addresses(&self) -> [u8; 8] {
        self.addresses
    }

    /// Whether this packet is a fragment: a later one, or the first of
    /// several.
    pub const fn is_fragment(&self) -> bool {
        self.fragment.more_fragments() || self.fragment.offset() != 0
    }

    /// The octets after the header, within the total length.
    pub const fn payload(&self) -> &'a [u8] {
        self.payload
    }
}

/// A checked IPv6 fixed header over the caller's packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ipv6Header<'a> {
    prefix: Ipv6HeaderPrefix,
    next_header: u8,
    addresses: [u8; 32],
    payload: &'a [u8],
}

impl<'a> Ipv6Header<'a> {
    /// Length of the fixed header.
    pub const LEN: usize = IPV6_LEN;

    /// Check the fixed header at the front of `packet`: version 6 and a
    /// payload length within `packet`.
    pub fn parse(packet: &'a [u8]) -> Result<Self, IpError> {
        let mut reader = Reader::new(packet);
        let fixed = reader.read::<IPV6_LEN>().map_err(|_| IpError::Truncated)?;
        let [
            first,
            second,
            _,
            _,
            length_high,
            length_low,
            next_header,
            _,
            addresses @ ..,
        ] = fixed;
        let prefix = Ipv6HeaderPrefix::from_bytes([first, second]);
        if prefix.version() != 6 {
            return Err(IpError::Version);
        }
        let payload_len = usize::from(u16::from_be_bytes([length_high, length_low]));
        let payload = reader.take(payload_len).map_err(|_| IpError::Length)?;
        Ok(Self {
            prefix,
            next_header,
            addresses,
            payload,
        })
    }

    pub const fn traffic_class(&self) -> DsField {
        DsField::from_bytes([self.prefix.traffic_class()])
    }

    pub const fn dscp(&self) -> Dscp {
        Dscp::from_field(self.traffic_class().dscp())
    }

    /// The Next Header field; extension headers are not followed.
    pub const fn next_header(&self) -> u8 {
        self.next_header
    }

    /// Source then destination address.
    pub const fn addresses(&self) -> [u8; 32] {
        self.addresses
    }

    /// The octets after the fixed header, within the payload length.
    pub const fn payload(&self) -> &'a [u8] {
        self.payload
    }
}

#[cfg(test)]
mod tests;
