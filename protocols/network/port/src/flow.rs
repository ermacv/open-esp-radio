//! Bounded, allocation-free classification; never protocol admission.
//!
//! Header boundaries follow RFC 791 §3.1 and RFC 8200 §3. Extension headers,
//! fragments and non-TCP/UDP traffic deliberately do not yield port keys.

use oer_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_network_wire::ip::{Ipv4Header, Ipv6Header};
use oer_network_wire::transport::Ports;

/// Directional transport identity within one Ethernet next hop.
///
/// Addresses and ports are retained exactly, without hash collisions. `None`
/// ports groups traffic by IP addresses and protocol only. Fragments stay in
/// that coarse queue, including the first fragment; they are not associated
/// with the corresponding unfragmented five-tuple. No reassembly or fragment
/// cache is maintained. Unsupported Ethernet encapsulations use `Opaque`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportFlow {
    Opaque,
    Ipv4 {
        addresses: [u8; 8],
        protocol: u8,
        ports: Option<[u8; 4]>,
    },
    Ipv6 {
        addresses: [u8; 32],
        protocol: u8,
        ports: Option<[u8; 4]>,
    },
}

impl TransportFlow {
    /// Inspect Ethernet-II headers without changing, accepting or rejecting
    /// the packet. Invalid/truncated input remains available to normal radio
    /// admission and is classified conservatively. Checksums are not checked.
    pub fn from_ethernet(frame: &[u8]) -> Self {
        let Ok((header, packet)) = EthernetHeader::parse(frame) else {
            return Self::Opaque;
        };
        match header.ether_type {
            EtherType::IPV4 => match Ipv4Header::parse(packet) {
                Ok(ip) => Self::Ipv4 {
                    addresses: ip.addresses(),
                    protocol: ip.protocol(),
                    ports: if ip.is_fragment() {
                        None
                    } else {
                        ports(ip.protocol(), ip.payload())
                    },
                },
                Err(_) => Self::Opaque,
            },
            EtherType::IPV6 => match Ipv6Header::parse(packet) {
                Ok(ip) => Self::Ipv6 {
                    addresses: ip.addresses(),
                    protocol: ip.next_header(),
                    ports: ports(ip.next_header(), ip.payload()),
                },
                Err(_) => Self::Opaque,
            },
            _ => Self::Opaque,
        }
    }
}

/// A malformed transport header groups its packet by addresses and protocol.
fn ports(protocol: u8, payload: &[u8]) -> Option<[u8; 4]> {
    Ports::parse(protocol, payload)
        .ok()
        .flatten()
        .map(Ports::to_bytes)
}

#[cfg(test)]
#[allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    reason = "tests assert classification and construct packet fixtures with slice copies"
)]
mod tests;
