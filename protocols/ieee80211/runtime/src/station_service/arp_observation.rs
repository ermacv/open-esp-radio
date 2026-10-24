//! ARP bytes remain local to the application sink. The public SN/PN lets an
//! application correlate an observed ARP with encrypted air traffic without
//! exporting any traffic key. No IP or EAPOL payload is projected.
use oer_ieee80211_wire::{
    ccmp::{CCMP_HEADER_LEN, CcmpHeader, CcmpPacketNumber},
    data::{EthernetFrameParts, LLC_SNAP_HEADER_LEN, rfc1042_ether_type},
    frame::DecodedHeader,
    sequence::SequenceNumber,
};

#[derive(Clone, Copy)]
pub struct ArpFrameObservation {
    pub transmitted: bool,
    pub group_addressed: bool,
    pub sequence: u16,
    pub packet_number: u64,
    pub key_id: u8,
    /// RFC 826 Ethernet/IPv4 ARP prefix, for validation by the application.
    pub payload: [u8; 28],
}
impl core::fmt::Debug for ArpFrameObservation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ArpFrameObservation")
            .field("transmitted", &self.transmitted)
            .field("sequence", &self.sequence)
            .field("packet_number", &self.packet_number)
            .field("key_id", &self.key_id)
            .field("payload", &"<redacted>")
            .finish()
    }
}
impl ArpFrameObservation {
    #[inline(never)]
    pub(super) fn transmit(bytes: &[u8], pn: CcmpPacketNumber) -> Option<Self> {
        let header = DecodedHeader::decode(bytes)?;
        let body = header.body().get(CCMP_HEADER_LEN..)?;
        let (llc, payload) = body.split_first_chunk::<LLC_SNAP_HEADER_LEN>()?;
        if rfc1042_ether_type(*llc) != Some(0x0806) {
            return None;
        }
        Some(Self {
            transmitted: true,
            group_addressed: false,
            sequence: SequenceNumber::from_sequence_control(header.sequence_control()).get(),
            packet_number: pn.value(),
            key_id: 0,
            payload: payload.get(..28)?.try_into().ok()?,
        })
    }
    #[inline(never)]
    pub(super) fn receive(bytes: &[u8], parts: &EthernetFrameParts<'_>) -> Option<Self> {
        if parts.ether_type != 0x0806 {
            return None;
        }
        let header = DecodedHeader::decode(bytes)?;
        if !header.control().protected() {
            return None;
        }
        let ccmp = CcmpHeader::parse(*header.body().first_chunk::<CCMP_HEADER_LEN>()?).ok()?;
        Some(Self {
            transmitted: false,
            group_addressed: parts.destination == [0xff; 6],
            sequence: SequenceNumber::from_sequence_control(header.sequence_control()).get(),
            packet_number: ccmp.packet_number().value(),
            key_id: ccmp.key_id().value(),
            payload: parts.payload.get(..28)?.try_into().ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_ieee80211_wire::{
        address::{Bssid, IndividualAddress, OwnAddress},
        ccmp::CcmpKeyId,
        station::StaProtectedDataFrame,
    };

    #[test]
    fn arp_metadata_uses_public_header_identity_and_redacts_local_payload() {
        let payload = *b"secret-local-arp-prefix-1234";
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
            let rx = ArpFrameObservation::receive(&bytes[..len], &parts).expect("validated ARP");
            assert!(!rx.transmitted);
            assert!(rx.group_addressed);
            assert_eq!(
                (rx.sequence, rx.packet_number, rx.key_id),
                (123, pn.value(), 2)
            );
            assert_eq!(rx.payload, payload);
            assert!(!std::format!("{rx:?}").contains("secret-local"));
            let allocated = CcmpPacketNumber::new(99).expect("TX PN");
            let tx = ArpFrameObservation::transmit(&bytes[..len], allocated).expect("prepared ARP");
            assert!(tx.transmitted);
            assert_eq!((tx.sequence, tx.packet_number, tx.key_id), (123, 99, 0));
            for end in 0..len {
                assert!(ArpFrameObservation::transmit(&bytes[..end], allocated).is_none());
            }
            assert!(
                ArpFrameObservation::receive(
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
            assert!(ArpFrameObservation::transmit(&bytes[..len], allocated).is_none());
        }
    }
}
