use super::*;
use oer_protocols_ieee80211_wire::frame::{FrameControlFields, MacHeader};

const A: [u8; 6] = [2, 0, 0, 0, 0, 1];
const B: [u8; 6] = [2, 0, 0, 0, 0, 2];

fn qos(tid: u8) -> QosControl {
    QosControl {
        tid: Tid::new(tid).unwrap(),
        service_period: ServicePeriod::Continues,
        body: QosBody::Msdu,
        high: 0,
    }
}

/// The header a frame of these fields decodes to, its receiver `receiver`.
fn header_to<R: ReceiverAddress>(
    frame_control: u16,
    receiver: [u8; 6],
    kind: impl FnOnce([u8; 6]) -> Option<R>,
    address4: Option<[u8; 6]>,
    qos: Option<QosControl>,
    ht_control: Option<[u8; 4]>,
) -> Option<HeaderSpec<R>> {
    let mut octets = std::vec::Vec::new();
    octets.extend(frame_control.to_le_bytes());
    octets.extend([0; 2]);
    octets.extend(receiver);
    octets.extend(A);
    octets.extend(B);
    octets.extend([0; 2]);
    octets.extend(address4.into_iter().flatten());
    octets.extend(
        qos.map(|qos| qos.field(QosAckPolicy::Normal).to_bytes())
            .into_iter()
            .flatten(),
    );
    octets.extend(ht_control.into_iter().flatten());
    let decoded = MacHeader::decode(&octets)?;
    (decoded.header_len() == octets.len()).then_some(())?;
    HeaderSpec::from_mac_header(decoded, kind)
}

fn header(
    frame_control: u16,
    address4: Option<[u8; 6]>,
    qos: Option<QosControl>,
    ht_control: Option<[u8; 4]>,
) -> Option<HeaderSpec<IndividualAddress>> {
    header_to(
        frame_control,
        B,
        IndividualAddress::new,
        address4,
        qos,
        ht_control,
    )
}

#[test]
fn a_receiver_address_is_individual_or_group_by_its_group_bit() {
    assert!(IndividualAddress::new(A).is_some());
    assert!(IndividualAddress::new([0xff; 6]).is_none());
    assert!(GroupAddress::new([0xff; 6]).is_some());
    assert!(GroupAddress::new(A).is_none());
}

#[test]
fn a_frame_control_is_a_data_or_management_frame_of_version_zero() {
    assert!(FrameControl::new(FrameControlFields::from_bytes([0x08, 0x00])).is_some()); // data
    assert!(FrameControl::new(FrameControlFields::from_bytes([0x80, 0x00])).is_some()); // beacon
    assert!(FrameControl::new(FrameControlFields::from_bytes([0x84, 0x00])).is_none()); // BlockAckReq, a control frame
    assert!(FrameControl::new(FrameControlFields::from_bytes([0x09, 0x00])).is_none()); // protocol version 1
    assert!(Tid::new(15).is_some());
    assert!(Tid::new(16).is_none());
}

#[test]
fn a_decoded_header_keeps_its_receiver_kind() {
    assert!(header(0x0088, None, Some(qos(1)), None).is_some());
    assert!(header_to(0x0088, B, GroupAddress::new, None, Some(qos(1)), None).is_none());
    assert!(
        header_to(
            0x0088,
            [0xff; 6],
            IndividualAddress::new,
            None,
            Some(qos(1)),
            None
        )
        .is_none()
    );
}

#[test]
fn the_qos_ack_policy_follows_the_frames_response() {
    let header = header(0x0388, Some(A), Some(qos(6)), None).unwrap();
    assert_eq!(header.header_len(), 32);
    let mut out = [0; 32];
    AckHeader::new(header).unwrap().encode(&mut out).unwrap();
    assert_eq!(out[30] & 0b0110_0000, 0, "Normal Ack");
    assert_eq!(out[30] & 0x0f, 6);
    NoAckHeader::new(header).unwrap().encode(&mut out).unwrap();
    assert_eq!(out[30] & 0b0110_0000, 0b0010_0000, "No Ack");
    assert_eq!(&out[4..10], &B);
}

#[test]
fn group_qos_frames_encode_no_ack_without_an_individual_response() {
    let group = GroupAddress::new([0xff; 6]).unwrap();
    let header = header_to(
        0x0288,
        group.get(),
        GroupAddress::new,
        None,
        Some(qos(3)),
        None,
    )
    .unwrap();
    let mut out = [0; 26];
    header.encode(&mut out).unwrap();
    assert_eq!(out[24] & 0b0110_0000, 0b0010_0000);
    assert_eq!(out[24] & 0x0f, 3);
    assert_eq!(&out[4..10], &group.get());
}

#[test]
fn only_a_frame_that_says_so_goes_without_an_ack() {
    // QoS data carries its Ack Policy, and Action No Ack is unacknowledged.
    assert!(NoAckHeader::new(header(0x0088, None, Some(qos(1)), None).unwrap()).is_some());
    assert!(NoAckHeader::new(header(0x00e0, None, None, None).unwrap()).is_some());
    // Non-QoS data and an Action frame are acknowledged by their receiver.
    assert!(NoAckHeader::new(header(0x0008, None, None, None).unwrap()).is_none());
    assert!(NoAckHeader::new(header(0x00d0, None, None, None).unwrap()).is_none());
    // An Action No Ack frame never solicits an ACK; the others may.
    assert!(AckHeader::new(header(0x00e0, None, None, None).unwrap()).is_none());
    assert!(AckHeader::new(header(0x00d0, None, None, None).unwrap()).is_some());
    assert!(AckHeader::new(header(0x0088, None, Some(qos(1)), None).unwrap()).is_some());
}

#[test]
fn a_header_decodes_as_it_was_encoded() {
    for (control, address4, qos, ht) in [
        // Management.
        (0x00b0, None, None, None),
        // QoS data to the DS.
        (0x0188, None, Some(qos(5)), None),
        // Four-address QoS data with HT Control.
        (0x8388, Some(A), Some(qos(3)), Some([1, 2, 3, 4])),
    ] {
        let spec = header(control, address4, qos, ht).unwrap();
        let mut bytes = [0; 36];
        AckHeader::new(spec).unwrap().encode(&mut bytes).unwrap();
        let decoded = HeaderSpec::from_mac_header(
            MacHeader::decode(&bytes[..spec.header_len()]).unwrap(),
            IndividualAddress::new,
        )
        .unwrap();
        for len in 0..spec.header_len() {
            let mut short = [0xa5; 36];
            assert_eq!(
                AckHeader::new(spec).unwrap().encode(&mut short[..len]),
                Err(super::StorageTooShort)
            );
            assert_eq!(short, [0xa5; 36]);
        }
        assert_eq!(decoded, spec);
    }
    // A group receiver is no individual header.
    let mut group = [0; 24];
    group[4..10].copy_from_slice(&[0xff; 6]);
    let group = MacHeader::decode(&group).unwrap();
    assert!(HeaderSpec::from_mac_header(group, IndividualAddress::new).is_none());
    assert!(HeaderSpec::from_mac_header(group, GroupAddress::new).is_some());
}

#[test]
fn a_decoded_qos_header_keeps_no_ack_policy() {
    let spec = header(0x0088, None, Some(qos(2)), None).unwrap();
    let mut bytes = [0; 26];
    NoAckHeader::new(spec).unwrap().encode(&mut bytes).unwrap();
    let decoded =
        HeaderSpec::from_mac_header(MacHeader::decode(&bytes).unwrap(), IndividualAddress::new)
            .unwrap();
    assert_eq!(decoded, spec);
    assert_eq!(decoded.qos(), Some(qos(2)));
}

/// Capabilities that carry an individual frame without an ACK only at
/// non-HT rates.
#[derive(Debug)]
struct NonHtNoAck;

impl Ieee80211Capabilities for NonHtNoAck {
    type SppScope = oer_protocols_ieee80211_wire::security::spp::Association;
    const CAPS: crate::LowerMacCapabilities = crate::LowerMacCapabilities {
        bands: crate::BandSet::GHZ2_4,
        formats: crate::FormatWidths {
            non_ht: crate::WidthSet::MHZ20,
            ht: crate::WidthSet::MHZ20,
            he: crate::WidthSet::MHZ20,
        },
        rates: crate::RateSupport {
            dsss_cck: crate::Support::Supported,
            ofdm: crate::Support::Supported,
            ht_max_mcs: oer_protocols_ieee80211_wire::phy::HtMcs::new(7),
            he_max_mcs: oer_protocols_ieee80211_wire::phy::HeMcs::new(9),

            he_dcm: crate::Support::Unsupported,
            he_ldpc: crate::Support::Unsupported,
            spatial_streams: 1,
        },
        services: crate::HardwareServices::FCS,
        vifs: 2,
        tx_queues: 4,
        max_mpdu_length: 2304,
        tx_power_ceiling_min_dbm: Some(0),
        individual_no_ack: crate::PhyFormatSet::NON_HT,
        station_receive_filters: crate::ReceiveFilter::BSS_MEMBER,
        access_point_receive_filters: crate::ReceiveFilter::BSS_MEMBER,
        rx_block_ack_max_tid: 7,
        rx_block_ack_max_window: 64,
        rx_amsdu_in_ampdu: true,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
        edca_max_txop_limit_units_32_us: u16::MAX,
    };
}

#[test]
fn a_port_rate_carries_no_ack_and_a_block_ack_request_only_where_the_rate_allows() {
    let legacy = PhyRate::Legacy(oer_protocols_ieee80211_wire::phy::LegacyRate::Ofdm6M);
    let ht = PhyRate::Ht(
        oer_protocols_ieee80211_wire::phy::HtRate::new(
            oer_protocols_ieee80211_wire::phy::HtMcs::new(7).unwrap(),
            oer_protocols_ieee80211_wire::phy::PpduBandwidth::Mhz20,
            oer_protocols_ieee80211_wire::phy::HtGuardInterval::Normal,
        )
        .unwrap(),
    );

    let rate = PortRate::<NonHtNoAck>::checked(legacy);
    assert_eq!(rate.no_ack().unwrap().rate().get(), legacy);
    assert_eq!(rate.legacy().unwrap().rate().get(), legacy);

    let rate = PortRate::<NonHtNoAck>::checked(ht);
    assert_eq!(rate.no_ack().unwrap_err().get(), ht);
    assert_eq!(rate.legacy().unwrap_err().get(), ht);
}

/// A TID is its four-bit field: 15 is held and read back, 16 is refused.
#[test]
fn a_tid_is_its_four_bit_field() {
    let highest = Tid::new(15).unwrap();
    assert_eq!(highest.get(), 15);
    assert_eq!(highest.wire().get(), 15);
    let control = QosControl {
        tid: highest,
        service_period: ServicePeriod::Continues,
        body: QosBody::Msdu,
        high: 0,
    };
    let field = control.field(oer_protocols_ieee80211_wire::frame::QosAckPolicy::Normal);
    assert_eq!(QosControl::from_field(field).tid, highest);
    assert_eq!(Tid::new(16), None);
}

/// An MPDU's length lies from an ACK's header to what the capabilities
/// buffer; a length past a 16-bit word is refused, not cut.
#[test]
fn an_mpdu_length_lies_between_an_ack_and_the_buffered_maximum() {
    let len = |octets| MpduLength::<NonHtNoAck>::new(octets).map(|len| len.get());
    assert_eq!(len(9), None);
    assert_eq!(len(10), Some(10));
    assert_eq!(len(2304), Some(2304));
    assert_eq!(len(2305), None);
    assert_eq!(len(65_536 + 100), None);
}
