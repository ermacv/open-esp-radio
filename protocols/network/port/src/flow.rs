//! Bounded, allocation-free classification; never protocol admission.
//!
//! Header boundaries follow RFC 791 §3.1 and RFC 8200 §3. Extension headers,
//! fragments and non-TCP/UDP traffic deliberately do not yield port keys.

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
        let Some((header, packet)) = frame.split_first_chunk::<{ super::ETHERNET_HEADER_LEN }>()
        else {
            return Self::Opaque;
        };
        match header {
            [.., 0x08, 0x00] => Self::ipv4(packet).unwrap_or(Self::Opaque),
            [.., 0x86, 0xdd] => Self::ipv6(packet).unwrap_or(Self::Opaque),
            _ => Self::Opaque,
        }
    }

    fn ipv4(packet: &[u8]) -> Option<Self> {
        let &[
            version_ihl,
            _,
            length_hi,
            length_lo,
            _,
            _,
            fragment_hi,
            fragment_lo,
            _,
            protocol,
            _,
            _,
            addresses @ ..,
        ] = packet.first_chunk::<20>()?;
        let header_len = usize::from(version_ihl & 0x0f).checked_mul(4)?;
        if version_ihl >> 4 != 4 || header_len < 20 {
            return None;
        }
        let total_len = usize::from(u16::from_be_bytes([length_hi, length_lo]));
        let payload = packet.get(..total_len)?.get(header_len..)?;
        let fragmented = u16::from_be_bytes([fragment_hi, fragment_lo]) & 0x3fff != 0;
        Some(Self::Ipv4 {
            addresses,
            protocol,
            ports: if fragmented {
                None
            } else {
                ports(protocol, payload)
            },
        })
    }

    fn ipv6(packet: &[u8]) -> Option<Self> {
        let (header, payload) = packet.split_first_chunk::<40>()?;
        let &[
            version,
            _,
            _,
            _,
            length_hi,
            length_lo,
            protocol,
            _,
            addresses @ ..,
        ] = header;
        if version >> 4 != 6 {
            return None;
        }
        let payload_len = usize::from(u16::from_be_bytes([length_hi, length_lo]));
        let payload = payload.get(..payload_len)?;
        Some(Self::Ipv6 {
            addresses,
            protocol,
            ports: ports(protocol, payload),
        })
    }
}

fn ports(protocol: u8, payload: &[u8]) -> Option<[u8; 4]> {
    match protocol {
        6 => {
            let header = payload.first_chunk::<20>()?;
            let header_len = usize::from(header[12] >> 4).checked_mul(4)?;
            if header_len < 20 || header_len > payload.len() {
                return None;
            }
        }
        17 => {
            let header = payload.first_chunk::<8>()?;
            let length = usize::from(u16::from_be_bytes([header[4], header[5]]));
            if length < 8 || length > payload.len() {
                return None;
            }
        }
        _ => return None,
    }
    payload.first_chunk::<4>().copied()
}

#[cfg(test)]
#[allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    reason = "tests assert classification and construct packet fixtures with slice copies"
)]
mod tests;
