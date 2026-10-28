//! ARP correlation of the protected link: the IPv4 addresses of an ARP MSDU
//! and the public header identity (SN, CCMP PN, key ID) of its MPDU, as the
//! observation events of `oer_ieee80211_instrumentation`. They let a
//! collector match an ARP exchange against encrypted air traffic without
//! any traffic key; no IP or EAPOL payload is projected. Only an observation
//! point calls these functions.
use oer_ieee80211_instrumentation::{ArpAddresses, ArpReceived, ArpTransmitted};
use oer_ieee80211_wire::{
    ccmp::{CCMP_HEADER_LEN, CcmpHeader, CcmpPacketNumber},
    data::{EthernetFrameParts, LLC_SNAP_HEADER_LEN, rfc1042_ether_type},
    frame::DecodedHeader,
    sequence::SequenceNumber,
};
use oer_ieee80211_wire::{ethertype::ETHER_TYPE_ARP, management::BROADCAST_ADDRESS};

/// The addresses and identity of the ARP request or reply a prepared
/// protected MPDU `bytes` carries, numbered `pn`; `None` for another MSDU.
pub fn transmitted_arp(
    bytes: &[u8],
    pn: CcmpPacketNumber,
) -> Option<(ArpAddresses, ArpTransmitted)> {
    let header = DecodedHeader::decode(bytes)?;
    let body = header.body().get(CCMP_HEADER_LEN..)?;
    let (llc, payload) = body.split_first_chunk::<LLC_SNAP_HEADER_LEN>()?;
    if rfc1042_ether_type(*llc) != Some(ETHER_TYPE_ARP) {
        return None;
    }
    let (operation, addresses) = ArpAddresses::parse(payload)?;
    Some((
        addresses,
        ArpTransmitted {
            operation,
            sequence: SequenceNumber::from_sequence_control(header.sequence_control()).get(),
            packet_number: pn.value(),
        },
    ))
}

/// The addresses and identity of the ARP request or reply in a received
/// protected MPDU `bytes`, whose MSDU is `parts`; `None` for another MSDU
/// or an unprotected MPDU.
pub fn received_arp(
    bytes: &[u8],
    parts: &EthernetFrameParts<'_>,
) -> Option<(ArpAddresses, ArpReceived)> {
    if parts.ether_type != ETHER_TYPE_ARP {
        return None;
    }
    let header = DecodedHeader::decode(bytes)?;
    if !header.control().protected() {
        return None;
    }
    let ccmp = CcmpHeader::parse(*header.body().first_chunk::<CCMP_HEADER_LEN>()?).ok()?;
    let (operation, addresses) = ArpAddresses::parse(parts.payload)?;
    Some((
        addresses,
        ArpReceived {
            operation,
            group_addressed: parts.destination == BROADCAST_ADDRESS,
            key_id: ccmp.key_id().value(),
            sequence: SequenceNumber::from_sequence_control(header.sequence_control()).get(),
            packet_number: ccmp.packet_number().value(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_ieee80211_instrumentation::ArpOperation;
    use oer_ieee80211_wire::{
        address::{Bssid, IndividualAddress, OwnAddress},
        ccmp::CcmpKeyId,
        station::StaProtectedDataFrame,
    };

    #[test]
    fn arp_identity_uses_public_header_fields_and_the_rfc_826_addresses() {
        // An Ethernet/IPv4 ARP request from 10.0.0.2 for 10.0.0.1.
        let mut payload = [0u8; 28];
        payload[..8].copy_from_slice(&[0, 1, 8, 0, 6, 4, 0, 1]);
        payload[14..18].copy_from_slice(&[10, 0, 0, 2]);
        payload[24..28].copy_from_slice(&[10, 0, 0, 1]);
        let addresses = ArpAddresses {
            sender: [10, 0, 0, 2],
            target: [10, 0, 0, 1],
        };
        let pn = CcmpPacketNumber::new(0x010203040506).expect("public PN");
        for peer_qos in [false, true] {
            let mut bytes = [0; 128];
            let frame = StaProtectedDataFrame {
                source: OwnAddress::new(
                    IndividualAddress::new([2, 0, 0, 0, 0, 1]).expect("station"),
                ),
                bssid: Bssid::new(IndividualAddress::new([2, 0, 0, 0, 0, 2]).expect("AP"))
                    .expect("BSSID"),
                destination: [0xff; 6],
                sequence_number: SequenceNumber::new(123).expect("SN"),
                user_priority: 0,
                peer_qos,
                ccmp_header: CcmpHeader::new(pn, CcmpKeyId::new(2).expect("GTK ID")).encode(),
                ether_type: 0x0806,
                payload: &payload,
            };
            let len = frame.encode(&mut bytes).expect("MPDU");
            let parts = EthernetFrameParts {
                source: [2, 0, 0, 0, 0, 2],
                destination: [0xff; 6],
                ether_type: 0x0806,
                payload: &payload,
            };
            assert_eq!(
                received_arp(&bytes[..len], &parts),
                Some((
                    addresses,
                    ArpReceived {
                        operation: ArpOperation::Request,
                        group_addressed: true,
                        key_id: 2,
                        sequence: 123,
                        packet_number: pn.value(),
                    }
                ))
            );
            let allocated = CcmpPacketNumber::new(99).expect("TX PN");
            assert_eq!(
                transmitted_arp(&bytes[..len], allocated),
                Some((
                    addresses,
                    ArpTransmitted {
                        operation: ArpOperation::Request,
                        sequence: 123,
                        packet_number: 99,
                    }
                ))
            );
            for end in 0..len {
                assert!(transmitted_arp(&bytes[..end], allocated).is_none());
            }
            assert!(
                received_arp(
                    &bytes[..len],
                    &EthernetFrameParts {
                        ether_type: 0x888e,
                        ..parts
                    }
                )
                .is_none()
            );
            let len = StaProtectedDataFrame {
                ether_type: 0x888e,
                ..frame
            }
            .encode(&mut bytes)
            .expect("EAPOL");
            assert!(transmitted_arp(&bytes[..len], allocated).is_none());
        }
    }
}
