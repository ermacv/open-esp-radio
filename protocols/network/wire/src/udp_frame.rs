//! The Ethernet, IPv4 and UDP headers of a datagram, as a template for the
//! datagrams of one flow: every field but the two lengths, the
//! identification and the two checksums is fixed, written once, and its
//! sum made once. A datagram's headers are the template's octets copied
//! where they go, those fields written in place.
//!
//! SOURCE: IEEE 802.3, 3.1.1 (MAC frame format); RFC 791, 3.1 (Internet
//! Header Format: Total Length at octet 2, Identification at 4, Header
//! Checksum at 10); RFC 768 (Length at octet 4, Checksum at 6, the pseudo
//! header); RFC 1071 (the sum).

use crate::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use crate::ethernet::{self, EthernetHeader};
use crate::ip::{IPV4_HEADER_LEN, Ipv4Fields};
use crate::transport::{PROTOCOL_UDP, Ports, UDP_HEADER_LEN, UdpHeader};

/// The octets of the three headers.
pub const UDP_FRAME_HEADERS: usize = ethernet::HEADER_LEN + IPV4_HEADER_LEN + UDP_HEADER_LEN;

const IP_AT: usize = ethernet::HEADER_LEN;
const TOTAL_LENGTH_AT: usize = IP_AT + 2;
const IDENTIFICATION_AT: usize = IP_AT + 4;
const IP_CHECKSUM_AT: usize = IP_AT + 10;
const UDP_AT: usize = IP_AT + IPV4_HEADER_LEN;
const UDP_LENGTH_AT: usize = UDP_AT + 4;
const UDP_CHECKSUM_AT: usize = UDP_AT + 6;

/// A datagram's data does not fit the lengths' fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TooLong;

/// The headers of one flow's datagrams ([`Self::new`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UdpFrameTemplate {
    /// The headers, the varying fields zero.
    octets: [u8; UDP_FRAME_HEADERS],
    /// The sum of the IPv4 header's fixed fields.
    ip: Checksum,
    /// The sum of the UDP pseudo header's and header's fixed fields.
    udp: Checksum,
}

impl UdpFrameTemplate {
    /// The template of `ethernet`, `ip` and `ports`; `ip`'s total length
    /// and identification are not taken.
    pub fn new(ethernet: &EthernetHeader, ip: &Ipv4Fields, ports: Ports) -> Self {
        let ip = Ipv4Fields {
            total_length: 0,
            identification: 0,
            ..*ip
        };
        let ip_octets = ip.with_checksum(0);
        let udp_octets = UdpHeader {
            ports,
            length: 0,
            checksum: 0,
        }
        .to_bytes();
        let mut octets = [0; UDP_FRAME_HEADERS];
        put(&mut octets, 0, ethernet.to_bytes());
        put(&mut octets, IP_AT, ip_octets);
        put(&mut octets, UDP_AT, udp_octets);
        Self {
            octets,
            ip: Checksum::new().with(&ip_octets),
            udp: Checksum::new()
                .with(&ipv4_pseudo_header(
                    ip.source,
                    ip.destination,
                    PROTOCOL_UDP,
                    0,
                ))
                .with(&udp_octets),
        }
    }

    /// Write the headers of a datagram of `length` octets of data, numbered
    /// `identification`, into `head`: the template's octets, then both
    /// lengths, the identification and the IPv4 checksum in place. The UDP
    /// checksum stays zero for [`Self::close`]; this returns the sum it
    /// starts from, the data's own still to add.
    ///
    /// # Errors
    /// [`TooLong`]: the lengths do not fit their fields; `head` is untouched.
    pub fn write(
        &self,
        head: &mut [u8; UDP_FRAME_HEADERS],
        length: usize,
        identification: u16,
    ) -> Result<Checksum, TooLong> {
        let udp_length =
            u16::try_from(UDP_HEADER_LEN.saturating_add(length)).map_err(|_| TooLong)?;
        let total_length = udp_length
            .checked_add(IPV4_HEADER_LEN as u16)
            .ok_or(TooLong)?;
        *head = self.octets;
        let total = total_length.to_be_bytes();
        let id = identification.to_be_bytes();
        let udp = udp_length.to_be_bytes();
        let checksum = self.ip.with(&total).with(&id).finish().to_be_bytes();
        put(head, TOTAL_LENGTH_AT, total);
        put(head, IDENTIFICATION_AT, id);
        put(head, IP_CHECKSUM_AT, checksum);
        put(head, UDP_LENGTH_AT, udp);
        // The length is in the pseudo header and the header both.
        Ok(self.udp.with(&udp).with(&udp))
    }

    /// Write the UDP checksum `sum` finishes into `head`, once the data it
    /// covers is summed.
    pub fn close(head: &mut [u8; UDP_FRAME_HEADERS], sum: Checksum) {
        put(head, UDP_CHECKSUM_AT, udp_field(sum.finish()).to_be_bytes());
    }
}

/// `octets` at `at`, a constant offset whose field lies inside the
/// headers.
fn put<const N: usize>(head: &mut [u8; UDP_FRAME_HEADERS], at: usize, octets: [u8; N]) {
    for (place, octet) in head.iter_mut().skip(at).zip(octets) {
        *place = octet;
    }
}

#[cfg(test)]
mod tests;
