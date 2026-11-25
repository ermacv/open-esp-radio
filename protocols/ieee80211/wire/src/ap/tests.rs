use crate::ap::profile::tests::TEST_ADVERTISEMENT;
use crate::management::DialogToken;
use crate::qos::Tid;

const TEST_HT_CAPABILITIES: crate::ht::HtLocalCapabilities = crate::ht::HtLocalCapabilities::new(
    crate::ht::HtCapabilityInfo::from_bytes([0x0c, 0x10]),
    0x03,
    0xff,
    0x01,
);

use super::*;
use crate::protection::ApBssProtection;
use crate::security::test_elements;
use crate::sequence::seq;

/// One MSDU to `destination` from `source`, with `ether_type`.
fn msdu<'p>(
    destination: [u8; 6],
    source: [u8; 6],
    ether_type: u16,
    payload: &'p [u8],
) -> crate::data::MsduParts<'p> {
    crate::data::MsduParts {
        destination: Eui48::new(destination),
        source: Eui48::new(source),
        ether_type: EtherType::new(ether_type),
        payload,
    }
}

#[test]
fn ap_eapol_data_frame_uses_from_ds_address_mapping() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut output = [0; 64];
    let len = ApDataFrame {
        access_point: Eui48::new(access_point),
        destination: Eui48::new(peer),
        sequence_number: seq(9),
        ether_type: EtherType::new(0x888e),
        payload: &[1, 2, 3],
    }
    .encode(&mut output)
    .unwrap();

    assert_eq!(len, 24 + 8 + 3);
    assert_eq!(&output[..2], &0x0208_u16.to_le_bytes());
    assert_eq!(&output[4..10], &peer);
    assert_eq!(&output[10..16], &access_point);
    assert_eq!(&output[16..22], &access_point);
    assert_eq!(&output[22..24], &0x0090_u16.to_le_bytes());
    assert_eq!(&output[24..32], &[0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e]);
    assert_eq!(&output[32..35], &[1, 2, 3]);
}

#[test]
fn protected_ap_frame_owns_from_ds_ccmp_and_plaintext_boundary() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let source = [2, 0, 0, 0, 0, 3];
    let ccmp = [3, 0, 0, 0x20, 0, 0, 0, 0];
    let mut output = [0; 96];
    let len = ApProtectedDataFrame {
        access_point: Eui48::new(access_point),
        peer: Eui48::new(peer),
        sequence_number: seq(7),
        user_priority: 0,
        peer_qos: crate::data::PeerQos::Qos,
        more_data: crate::data::MoreData::Clear,
        ccmp_header: ccmp,
        msdu: msdu(peer, source, 0x0800, &[1, 2, 3, 4]),
    }
    .encode(&mut output)
    .unwrap();

    assert_eq!(len, 26 + 8 + 8 + 4);
    assert_eq!(&output[..2], &0x4288_u16.to_le_bytes());
    assert_eq!(&output[4..10], &peer);
    assert_eq!(&output[10..16], &access_point);
    assert_eq!(&output[16..22], &source);
    assert_eq!(&output[22..24], &0x0070_u16.to_le_bytes());
    assert_eq!(&output[26..34], &ccmp);
    assert_eq!(&output[34..42], &[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0]);
    assert_eq!(&output[42..46], &[1, 2, 3, 4]);
}

/// The in-place prefix is the frame's own octets in front of its payload,
/// its CCMP header reserved: a QoS peer's 42, a group or non-QoS frame's 40.
#[test]
fn an_in_place_prefix_is_the_protected_frame_up_to_its_payload() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let source = [2, 0, 0, 0, 0, 3];
    for (peer, qos) in [
        ([2, 0, 0, 0, 0, 2], crate::data::PeerQos::Qos),
        ([1, 0, 0x5e, 0, 0, 1], crate::data::PeerQos::NonQos),
    ] {
        let payload = [1, 2, 3, 4];
        let mut whole = [0; 96];
        let len = ApProtectedDataFrame {
            access_point: Eui48::new(access_point),
            peer: Eui48::new(peer),
            sequence_number: seq(7),
            user_priority: 0,
            peer_qos: qos,
            more_data: crate::data::MoreData::Clear,
            ccmp_header: [0; 8],
            msdu: msdu(peer, source, 0x0800, &payload),
        }
        .encode(&mut whole)
        .unwrap();
        let prefix = ApProtectedDataPrefix {
            access_point: Eui48::new(access_point),
            peer: Eui48::new(peer),
            sequence_number: seq(7),
            user_priority: 0,
            peer_qos: qos,
            more_data: crate::data::MoreData::Clear,
            msdu: msdu(peer, source, 0x0800, &[]).header(),
        };
        let at = prefix.prefix_len().unwrap();
        assert_eq!(
            at,
            match qos {
                crate::data::PeerQos::Qos => 42,
                crate::data::PeerQos::NonQos => 40,
            }
        );
        let mut output = [0; 96];
        output[at..at + 4].copy_from_slice(&payload);
        assert_eq!(prefix.encode(&mut output[..at]), Ok(at));
        assert_eq!(&output[..len], &whole[..len]);
        // A region other than the prefix's length is refused, unwritten.
        let mut short = [0xa5; 96];
        assert!(prefix.encode(&mut short[..at - 1]).is_err());
        assert!(short.iter().all(|octet| *octet == 0xa5));
    }
}

#[test]
fn ap_amsdu_encodes_multiple_open_and_ccmp_subframes_in_order() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let frames = [
        msdu(peer, [2, 0, 0, 0, 0, 3], 0x0800, &[1, 2, 3]),
        msdu(peer, [2, 0, 0, 0, 0, 4], 0x0806, &[4, 5, 6, 7]),
        msdu(peer, [2, 0, 0, 0, 0, 5], 0x86dd, &[8, 9]),
    ];
    let ccmp = [9, 0, 0, 0x20, 0, 0, 0, 0];
    let mut protected = [0xa5; 192];
    let protected_len = ApAmsduFrame {
        access_point: Eui48::new(access_point),
        peer: Eui48::new(peer),
        sequence_number: seq(11),
        user_priority: 5,
        more_data: crate::data::MoreData::Buffered,
        ccmp_header: Some(ccmp),
        msdus: &frames,
    }
    .encode(&mut protected)
    .unwrap();
    assert_eq!(
        protected_len,
        ap_amsdu_frame_length(&frames, crate::security::LinkProtection::Ccmp).unwrap()
    );
    assert_eq!(&protected[..2], &0x6288_u16.to_le_bytes());
    assert_eq!(&protected[4..10], &peer);
    assert_eq!(&protected[22..24], &0x00b0_u16.to_le_bytes());
    assert_eq!(protected[24], 0x85);
    assert_eq!(&protected[26..34], &ccmp);
    // Independent reference: the first subframe's DA, SA, MSDU length 11,
    // RFC 1042 LLC/SNAP with IPv4, payload and one padding octet.
    assert_eq!(
        &protected[34..60],
        &[
            2, 0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 3, 0, 11, 0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2, 3,
            0
        ]
    );
    let mut decoded = crate::data::amsdu_subframes(
        DataInterfaceRole::Station,
        &protected[..protected_len],
        crate::data::IEEE80211_QOS_DATA_HEADER_LEN + CCMP_HEADER_LEN,
        protected_len - crate::data::IEEE80211_QOS_DATA_HEADER_LEN - CCMP_HEADER_LEN,
    )
    .unwrap();
    assert_eq!(decoded.next().unwrap().unwrap().payload, &[1, 2, 3]);
    assert_eq!(decoded.next().unwrap().unwrap().payload, &[4, 5, 6, 7]);
    assert_eq!(decoded.next().unwrap().unwrap().payload, &[8, 9]);
    assert!(decoded.next().is_none());

    let mut open = [0; 192];
    let open_len = ApAmsduFrame {
        access_point: Eui48::new(access_point),
        peer: Eui48::new(peer),
        sequence_number: seq(12),
        user_priority: 0,
        more_data: crate::data::MoreData::Clear,
        ccmp_header: None,
        msdus: &frames,
    }
    .encode(&mut open)
    .unwrap();
    assert_eq!(open_len, protected_len - CCMP_HEADER_LEN);
    assert_eq!(&open[..2], &0x0288_u16.to_le_bytes());
    assert_eq!(open[24], 0x80);
}

#[test]
fn ap_amsdu_fails_before_mutating_output_on_peer_or_capacity_miss() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let first = msdu(peer, [0; 6], 0, &[]);
    let wrong_peer = msdu([2, 0, 0, 0, 0, 9], [0; 6], 0, &[]);
    let frames = [first, wrong_peer];
    let mut output = [0xa5; 80];
    assert_eq!(
        ApAmsduFrame {
            access_point: Eui48::new(access_point),
            peer: Eui48::new(peer),
            sequence_number: seq(0),
            user_priority: 0,
            more_data: crate::data::MoreData::Clear,
            ccmp_header: Some([0; 8]),
            msdus: &frames,
        }
        .encode(&mut output),
        Err(ApDataFrameError::InvalidPeer)
    );
    assert_eq!(output, [0xa5; 80]);

    let frames = [first, first];
    let required = ap_amsdu_frame_length(&frames, crate::security::LinkProtection::Ccmp).unwrap();
    let mut short = [0x5a; 64];
    assert!(short.len() < required);
    assert_eq!(
        ApAmsduFrame {
            access_point: Eui48::new(access_point),
            peer: Eui48::new(peer),
            sequence_number: seq(0),
            user_priority: 0,
            more_data: crate::data::MoreData::Clear,
            ccmp_header: Some([0; 8]),
            msdus: &frames,
        }
        .encode(&mut short),
        Err(ApDataFrameError::OutputTooSmall { required })
    );
    assert_eq!(short, [0x5a; 64]);
}

#[test]
fn protected_ap_frame_rejects_a_destination_outside_pairwise_owner() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut output = [0; 64];
    assert_eq!(
        ApProtectedDataFrame {
            access_point: Eui48::new(access_point),
            peer: Eui48::new(peer),
            sequence_number: seq(0),
            user_priority: 0,
            peer_qos: crate::data::PeerQos::NonQos,
            more_data: crate::data::MoreData::Clear,
            ccmp_header: [0; 8],
            msdu: msdu([2, 0, 0, 0, 0, 9], [0; 6], 0, &[]),
        }
        .encode(&mut output),
        Err(ApDataFrameError::InvalidPeer)
    );
}

#[test]
fn ap_action_frame_and_parser_preserve_per_peer_addba_identity() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let body = [3, 1, 7, 0, 0, 0x02, 0x08, 0, 0];
    let mut frame = [0_u8; 40];
    let length = ApManagementFrame {
        subtype: ApManagementSubtype::Action,
        access_point: Eui48::new(access_point),
        peer: Eui48::new(peer),
        sequence_number: seq(9),
        body: &body,
    }
    .encode(&mut frame)
    .unwrap();
    // Reverse direction for the peer-originated response parsed by AP.
    frame[4..10].copy_from_slice(&access_point);
    frame[10..16].copy_from_slice(&peer);
    assert!(matches!(
        parse_ap_management_request(&TEST_ADVERTISEMENT, &frame[..length], Eui48::new(access_point)),
        Some(ApManagementRequest::BlockAck {
            peer: parsed_peer,
            action: BlockAckAction::AddbaResponse {
                dialog_token,
                parameters,
                ..
            },
        }) if dialog_token == DialogToken::new(7) && parsed_peer == Eui48::new(peer) && (parameters.tid(), parameters.window()) == (Tid::constant::<0>(), 32)
    ));
}

#[test]
fn association_response_owns_status_aid_and_ht_channel_capability() {
    let mut body = [0; AP_ASSOCIATION_RESPONSE_BODY_LEN];
    let ht20 = crate::channel::Channel::ghz2_4(6, crate::channel::ChannelWidth::Mhz20).unwrap();
    write_ht_association_response(
        &TEST_ADVERTISEMENT,
        &mut body,
        crate::status::StatusCode::DENIED_NO_MORE_STAS,
        crate::management::AssociationId::new(0x0123),
        ht20,
        None,
        ApBssProtection::default(),
    )
    .unwrap();
    assert_eq!(&body[2..4], &17_u16.to_le_bytes());
    assert_eq!(&body[4..6], &[0, 0]);
    write_ht_association_response(
        &TEST_ADVERTISEMENT,
        &mut body,
        crate::status::StatusCode::SUCCESS,
        crate::management::AssociationId::new(1),
        ht20,
        None,
        ApBssProtection::default(),
    )
    .unwrap();
    assert_eq!(&body[4..6], &0xc001_u16.to_le_bytes());
    assert!(body.windows(2).any(|window| window == [45, 26]));
    assert!(body.windows(3).any(|window| window == [61, 22, 6]));

    let mut peer_ht_record = crate::ht::ht_capability_ie(TEST_HT_CAPABILITIES, ht20);
    peer_ht_record[4] = 0x17;
    let peer_ht = ht_peer_capabilities(&peer_ht_record).unwrap();
    write_ht_association_response(
        &TEST_ADVERTISEMENT,
        &mut body,
        crate::status::StatusCode::SUCCESS,
        crate::management::AssociationId::new(1),
        ht20,
        Some(peer_ht),
        ApBssProtection::default(),
    )
    .unwrap();
    assert_eq!(
        body[AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN + 4],
        0x17,
        "association response must carry the vendor-negotiated peer spacing"
    );

    assert_eq!(
        write_ht_association_response(
            &TEST_ADVERTISEMENT,
            &mut body,
            crate::status::StatusCode::SUCCESS,
            None,
            ht20,
            None,
            ApBssProtection::default(),
        ),
        Err(ApAssociationResponseError::MissingAssociationId)
    );
    let ht40 =
        crate::channel::Channel::ghz2_4(6, crate::channel::ChannelWidth::Mhz40Below).unwrap();
    write_ht_association_response(
        &TEST_ADVERTISEMENT,
        &mut body,
        crate::status::StatusCode::SUCCESS,
        crate::management::AssociationId::new(1),
        ht40,
        None,
        ApBssProtection::default(),
    )
    .unwrap();
    assert!(body.windows(4).any(|window| window == [45, 26, 0x6e, 0x10]));
    assert!(body.windows(4).any(|window| window == [61, 22, 6, 0x07]));
    let ht_capability = AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN;
    assert_eq!(
        body[ht_capability + crate::ht::HtDuplicateMcs32::CAPABILITY_IE_BYTE]
            & crate::ht::HtDuplicateMcs32::CAPABILITY_IE_MASK,
        0,
        "the AP response must not advertise unqualified local MCS32 reception"
    );
    assert_eq!(
        body[ht_capability + 17],
        0x01,
        "the AP response must keep the implemented TX/RX MCS0..MCS7 sets equal"
    );
}

#[test]
fn peer_disconnect_frames_own_subtype_reason_and_sequence() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut output = [0; AP_PEER_DISCONNECT_LEN];
    assert_eq!(
        write_ap_peer_disconnect(
            &mut output,
            Eui48::new(access_point),
            Eui48::new(peer),
            ApPeerDisconnectKind::Disassociation,
            crate::status::ReasonCode::new(4),
            seq(7),
        ),
        Ok(AP_PEER_DISCONNECT_LEN),
    );
    assert_eq!(&output[..2], &0x00a0_u16.to_le_bytes());
    assert_eq!(&output[4..10], &peer);
    assert_eq!(&output[10..16], &access_point);
    assert_eq!(&output[22..24], &0x0070_u16.to_le_bytes());
    assert_eq!(&output[24..26], &4_u16.to_le_bytes());

    write_ap_peer_disconnect(
        &mut output,
        Eui48::new(access_point),
        Eui48::new(peer),
        ApPeerDisconnectKind::Deauthentication,
        crate::status::ReasonCode::new(2),
        seq(8),
    )
    .unwrap();
    assert_eq!(&output[..2], &0x00c0_u16.to_le_bytes());
    assert_eq!(&output[24..26], &2_u16.to_le_bytes());
}

#[test]
fn tim_bitmap_update_matches_aid_bit_selection() {
    use crate::beacon::TimVirtualBitmap;
    use crate::management::AssociationId;

    let mut bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
    bitmap
        .set(
            AssociationId::new(7).unwrap(),
            crate::station_beacon::BufferedTraffic::Buffered,
        )
        .unwrap();
    bitmap
        .set(
            AssociationId::new(8).unwrap(),
            crate::station_beacon::BufferedTraffic::Buffered,
        )
        .unwrap();
    bitmap
        .set(
            AssociationId::new(15).unwrap(),
            crate::station_beacon::BufferedTraffic::Buffered,
        )
        .unwrap();
    assert_eq!(bitmap.partial().bitmap_offset(), 0);
    assert_eq!(bitmap.partial().octets(), &[0x80, 0x81]);
}

#[test]
fn observes_only_to_ds_data_power_state() {
    let peer = [1, 2, 3, 4, 5, 6];
    let mut frame = [0_u8; 24];
    frame[10..16].copy_from_slice(&peer);
    frame[..2].copy_from_slice(&0x1108_u16.to_le_bytes());
    assert_eq!(
        observe_ap_power_save(&frame),
        Some(ApPowerSaveObservation::Sleeping {
            peer: Eui48::new(peer)
        })
    );
    frame[..2].copy_from_slice(&0x0108_u16.to_le_bytes());
    assert_eq!(
        observe_ap_power_save(&frame),
        Some(ApPowerSaveObservation::Active {
            peer: Eui48::new(peer)
        })
    );
    frame[..2].copy_from_slice(&0x0008_u16.to_le_bytes());
    assert_eq!(observe_ap_power_save(&frame), None);
}

#[test]
fn ps_poll_owns_peer_and_association_id() {
    let peer = [1, 2, 3, 4, 5, 6];
    let mut frame = [0_u8; 16];
    frame[..2].copy_from_slice(&0x00a4_u16.to_le_bytes());
    frame[2..4].copy_from_slice(&0xc123_u16.to_le_bytes());
    frame[10..16].copy_from_slice(&peer);
    assert_eq!(
        observe_ap_power_save(&frame),
        Some(ApPowerSaveObservation::PsPoll {
            peer: Eui48::new(peer),
            association_id: crate::management::AidField::from_bytes([0x23, 0xc1])
        })
    );
}

#[test]
fn sae_authentication_frames_cross_between_station_and_access_point() {
    use crate::station::{SaeAuthenticationFrame, parse_sae_authentication};

    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let commit = [0x13, 0, 0xaa, 0xbb];
    let mut request = [0_u8; 64];
    let len = SaeAuthenticationFrame {
        source: crate::address::OwnAddress::new(
            oer_protocols_ieee802_wire::IndividualAddress::new(Eui48::new(peer)).unwrap(),
        ),
        bssid: crate::address::Bssid::new(
            oer_protocols_ieee802_wire::IndividualAddress::new(Eui48::new(access_point)).unwrap(),
        )
        .unwrap(),
        sequence_number: seq(1),
        transaction: crate::management::AuthenticationTransaction::new(1),
        status_code: crate::status::StatusCode::SAE_HASH_TO_ELEMENT,
        body: &commit,
    }
    .encode(&mut request)
    .unwrap();
    assert_eq!(
        parse_ap_management_request(
            &TEST_ADVERTISEMENT,
            &request[..len],
            Eui48::new(access_point)
        ),
        Some(ApManagementRequest::SaeAuthentication {
            peer: Eui48::new(peer),
            transaction: crate::management::AuthenticationTransaction::new(1),
            status: crate::status::StatusCode::SAE_HASH_TO_ELEMENT,
            body: &commit,
        })
    );

    let confirm = [1, 0, 0xcc];
    let mut response = [0_u8; 64];
    let len = write_sae_authentication(
        &mut response,
        Eui48::new(access_point),
        Eui48::new(peer),
        crate::management::AuthenticationTransaction::new(2),
        crate::status::StatusCode::SUCCESS,
        &confirm,
        seq(2),
    )
    .unwrap();
    let parsed =
        parse_sae_authentication(&response[..len], Eui48::new(peer), Eui48::new(access_point))
            .unwrap();
    assert_eq!(
        parsed.transaction,
        crate::management::AuthenticationTransaction::new(2)
    );
    assert_eq!(parsed.status_code, crate::status::StatusCode::SUCCESS);
    assert_eq!(parsed.body, confirm);
    assert_eq!(
        write_sae_authentication(
            &mut response[..32],
            Eui48::new(access_point),
            Eui48::new(peer),
            crate::management::AuthenticationTransaction::new(2),
            crate::status::StatusCode::SUCCESS,
            &confirm,
            seq(2)
        ),
        Err(ApAssociationResponseError::OutputTooSmall { required: 33 })
    );
}

#[test]
fn parses_only_requests_for_the_owned_bssid() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut authentication = [0_u8; 30];
    authentication[..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
    authentication[4..10].copy_from_slice(&access_point);
    authentication[10..16].copy_from_slice(&peer);
    authentication[16..22].copy_from_slice(&access_point);
    authentication[26..28].copy_from_slice(&1_u16.to_le_bytes());
    assert_eq!(
        parse_ap_management_request(
            &TEST_ADVERTISEMENT,
            &authentication,
            Eui48::new(access_point)
        ),
        Some(ApManagementRequest::OpenAuthentication {
            peer: Eui48::new(peer)
        })
    );
    authentication[4] ^= 1;
    assert_eq!(
        parse_ap_management_request(
            &TEST_ADVERTISEMENT,
            &authentication,
            Eui48::new(access_point)
        ),
        None
    );
}

#[test]
fn association_retains_the_highest_common_advertised_legacy_rate() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut association = [0_u8; 42];
    association[4..10].copy_from_slice(&access_point);
    association[10..16].copy_from_slice(&peer);
    association[16..22].copy_from_slice(&access_point);
    association[28..34].copy_from_slice(&[1, 4, 0x82, 0x84, 0x0c, 0x30]);
    association[34..39].copy_from_slice(&[50, 3, 0x48, 0x6c, 0x7f]);
    association[39..42].copy_from_slice(&[48, 1, 0]);
    assert_eq!(
        parse_ap_management_request(&TEST_ADVERTISEMENT, &association, Eui48::new(access_point)),
        Some(ApManagementRequest::Association {
            peer: Eui48::new(peer),
            security: ApAssociationSecurityObservation {
                capability: crate::management::CapabilityInformation::from_bytes([
                    association[24],
                    association[25],
                ]),
                rsn_ie: crate::scan::ElementOccurrence::Once(&association[39..42]),
                rsnxe: crate::scan::ElementOccurrence::Absent,
                legacy_wpa: crate::scan::ElementPresence::Absent,
                information_elements: crate::scan::ElementList::Complete,
            },
            maximum_legacy_rate_500kbps: 108,
            ht_capabilities: None,
            qos: crate::data::PeerQos::NonQos,
            preamble: crate::phy::DsssPreamble::Long,
        })
    );
}

#[test]
fn association_retains_exact_rsnxe_and_duplicate_count() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut association = [0_u8; 38];
    association[4..10].copy_from_slice(&access_point);
    association[10..16].copy_from_slice(&peer);
    association[16..22].copy_from_slice(&access_point);
    association[24..26].copy_from_slice(&0x0010_u16.to_le_bytes());
    association[28..31].copy_from_slice(&[48, 1, 0]);
    association[31..34].copy_from_slice(&[244, 1, 0x20]);
    association[34..38].copy_from_slice(&[244, 2, 0x40, 0x00]);

    let Some(ApManagementRequest::Association { security, .. }) =
        parse_ap_management_request(&TEST_ADVERTISEMENT, &association, Eui48::new(access_point))
    else {
        panic!("association request must parse");
    };
    assert_eq!(
        security.rsn_ie,
        crate::scan::ElementOccurrence::Once(&association[28..31])
    );
    assert_eq!(security.rsnxe, crate::scan::ElementOccurrence::Repeated);
    assert_eq!(
        security.information_elements,
        crate::scan::ElementList::Complete
    );
}

#[test]
fn association_retains_the_peers_complete_ht40_receive_facts() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut association = [0_u8; 62];
    association[4..10].copy_from_slice(&access_point);
    association[10..16].copy_from_slice(&peer);
    association[16..22].copy_from_slice(&access_point);
    association[28..34].copy_from_slice(&[1, 4, 0x82, 0x84, 0x0c, 0x6c]);
    let channel =
        crate::channel::Channel::ghz2_4(6, crate::channel::ChannelWidth::Mhz40Above).unwrap();
    association[34..].copy_from_slice(&crate::ht::ht_capability_ie(TEST_HT_CAPABILITIES, channel));

    let Some(ApManagementRequest::Association {
        maximum_legacy_rate_500kbps,
        ht_capabilities: Some(ht),
        qos,
        ..
    }) = parse_ap_management_request(&TEST_ADVERTISEMENT, &association, Eui48::new(access_point))
    else {
        panic!("complete HT40 association request must parse");
    };
    assert_eq!(maximum_legacy_rate_500kbps, 108);
    assert!(ht.supports_40_mhz());
    assert!(ht.supports_short_guard_interval(crate::channel::ChannelWidth::Mhz40Above));
    assert_eq!(ht.highest_rx_mcs(), 7);
    assert_eq!(qos, crate::data::PeerQos::Qos);
}

#[test]
fn complete_response_encoders_own_addresses_sequence_and_status() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut authentication = [0xaa; AP_AUTHENTICATION_RESPONSE_LEN];
    write_open_authentication_response(
        &mut authentication,
        Eui48::new(access_point),
        Eui48::new(peer),
        crate::status::StatusCode::DENIED_NO_MORE_STAS,
        seq(7),
    )
    .unwrap();
    assert_eq!(&authentication[4..10], &peer);
    assert_eq!(&authentication[10..16], &access_point);
    assert_eq!(&authentication[26..28], &2_u16.to_le_bytes());
    assert_eq!(&authentication[28..30], &17_u16.to_le_bytes());

    let mut association = [0; AP_ASSOCIATION_RESPONSE_LEN];
    write_ht_association_response_frame_for_security(
        &TEST_ADVERTISEMENT,
        &mut association,
        Eui48::new(access_point),
        Eui48::new(peer),
        crate::status::StatusCode::SUCCESS,
        crate::management::AssociationId::new(1),
        seq(8),
        crate::channel::Channel::ghz2_4(6, crate::channel::ChannelWidth::Mhz20).unwrap(),
        None,
        test_elements::WPA2_PERSONAL,
        ApBssProtection::default(),
    )
    .unwrap();
    assert_eq!(&association[..2], &0x0010_u16.to_le_bytes());
    assert_eq!(&association[22..24], &0x0080_u16.to_le_bytes());
    assert_eq!(&association[28..30], &0xc001_u16.to_le_bytes());
}

#[test]
fn a_5_ghz_association_response_carries_no_erp_element() {
    let mut body = [0; AP_ASSOCIATION_RESPONSE_BODY_LEN];
    let ghz2_4 = crate::channel::Channel::ghz2_4(6, crate::channel::ChannelWidth::Mhz20).unwrap();
    let ghz5 = crate::channel::Channel::ghz5(36, crate::channel::ChannelWidth::Mhz20).unwrap();
    let length = |channel| {
        let mut body = [0; AP_ASSOCIATION_RESPONSE_BODY_LEN];
        let length = write_ht_association_response(
            &TEST_ADVERTISEMENT,
            &mut body,
            crate::status::StatusCode::SUCCESS,
            crate::management::AssociationId::new(1),
            channel,
            None,
            ApBssProtection::default(),
        )
        .unwrap();
        (length, body)
    };
    let (long, _) = length(ghz2_4);
    assert_eq!(long, AP_ASSOCIATION_RESPONSE_BODY_LEN);
    let (short, written) = length(ghz5);
    assert_eq!(short, AP_ASSOCIATION_RESPONSE_BODY_LEN - 3);
    body.copy_from_slice(&written);
    // Rates, extended rates, then the WMM element: no ERP between them.
    assert_eq!(body[22], 221);
    assert!(
        body[..short]
            .windows(3)
            .any(|window| window == [61, 22, 36])
    );
}

#[test]
fn protected_management_codecs_keep_the_body_after_the_ccmp_header() {
    use crate::{
        ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber},
        management_protection::SaQuery,
    };
    let ap = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let ccmp = CcmpHeader::new(CcmpPacketNumber::new(7).unwrap(), CcmpKeyId::PAIRWISE).encode();
    let query = SaQuery::Request {
        transaction: crate::management_protection::SaQueryTransaction::from_bytes([0x12, 0x34]),
    }
    .encode();
    let mut frame = [0; 64];
    let length = ApProtectedManagementFrame {
        subtype: ApManagementSubtype::Action,
        access_point: Eui48::new(ap),
        peer: Eui48::new(peer),
        sequence_number: seq(9),
        ccmp_header: ccmp,
        body: &query,
    }
    .encode(&mut frame)
    .unwrap();
    assert_eq!(frame[1] & 0x40, 0x40);
    assert_eq!(&frame[24..32], &ccmp);
    assert_eq!(&frame[32..length], &query);
    frame[4..10].copy_from_slice(&ap);
    frame[10..16].copy_from_slice(&peer);
    assert_eq!(
        parse_ap_management_request(&TEST_ADVERTISEMENT, &frame[..length], Eui48::new(ap)),
        Some(ApManagementRequest::SaQuery {
            peer: Eui48::new(peer),
            query: SaQuery::Request {
                transaction: crate::management_protection::SaQueryTransaction::from_bytes([
                    0x12, 0x34
                ])
            }
        })
    );
    for subtype in [
        ApManagementSubtype::Deauthentication,
        ApManagementSubtype::Disassociation,
    ] {
        let length = ApProtectedManagementFrame {
            subtype,
            access_point: Eui48::new(ap),
            peer: Eui48::new(peer),
            sequence_number: seq(10),
            ccmp_header: ccmp,
            body: &[3, 0],
        }
        .encode(&mut frame)
        .unwrap();
        frame[4..10].copy_from_slice(&ap);
        frame[10..16].copy_from_slice(&peer);
        assert!(matches!(
            parse_ap_management_request(&TEST_ADVERTISEMENT, &frame[..length], Eui48::new(ap)),
            Some(
                ApManagementRequest::Deauthentication {
                    reason: crate::status::ReasonCode::DEAUTH_LEAVING,
                    ..
                } | ApManagementRequest::Disassociation {
                    reason: crate::status::ReasonCode::DEAUTH_LEAVING,
                    ..
                }
            )
        ));
        assert!(
            parse_ap_management_request(&TEST_ADVERTISEMENT, &frame[..31], Eui48::new(ap))
                .is_none()
        );
    }
}

#[test]
fn ap_management_encoders_admit_complete_capacity_before_writing() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let sequence_number = seq(0);
    let plain = ApManagementFrame {
        access_point: Eui48::new(access_point),
        peer: Eui48::new(peer),
        sequence_number,
        subtype: ApManagementSubtype::Action,
        body: &[3, 1, 0],
    };
    let protected = ApProtectedManagementFrame {
        access_point: Eui48::new(access_point),
        peer: Eui48::new(peer),
        sequence_number,
        subtype: ApManagementSubtype::Action,
        ccmp_header: [0; CCMP_HEADER_LEN],
        body: &[3, 1, 0],
    };
    type Encoder<'a> = dyn Fn(&mut [u8]) -> Result<usize, ApAssociationResponseError> + 'a;
    let encoders: [&Encoder<'_>; 5] = [
        &|out| plain.encode(out),
        &|out| protected.encode(out),
        &|out| {
            write_open_authentication_response(
                out,
                Eui48::new(access_point),
                Eui48::new(peer),
                crate::status::StatusCode::SUCCESS,
                sequence_number,
            )
        },
        &|out| {
            write_sae_authentication(
                out,
                Eui48::new(access_point),
                Eui48::new(peer),
                crate::management::AuthenticationTransaction::new(1),
                crate::status::StatusCode::SUCCESS,
                &[1, 2, 3],
                sequence_number,
            )
        },
        &|out| {
            write_ap_peer_disconnect(
                out,
                Eui48::new(access_point),
                Eui48::new(peer),
                ApPeerDisconnectKind::Deauthentication,
                crate::status::ReasonCode::UNSPECIFIED,
                sequence_number,
            )
        },
    ];
    for encode in encoders {
        let mut complete = [0xa5; 128];
        let required = encode(&mut complete).unwrap();
        for capacity in 0..required {
            let mut short = [0xa5; 128];
            assert_eq!(
                encode(&mut short[..capacity]),
                Err(ApAssociationResponseError::OutputTooSmall { required })
            );
            assert_eq!(short, [0xa5; 128]);
        }
        assert!(complete[required..].iter().all(|byte| *byte == 0xa5));
    }
}

/// Each Frame Control bit an access point refuses in a peer's management
/// request refuses it alone: protocol version, To DS, From DS; and in a
/// Probe Request also More Fragments, Protected and a fragment number.
#[test]
fn each_refused_frame_control_bit_refuses_a_management_request() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut authentication = [0_u8; 30];
    authentication[..2].copy_from_slice(&[0xb0, 0x00]);
    authentication[4..10].copy_from_slice(&access_point);
    authentication[10..16].copy_from_slice(&peer);
    authentication[16..22].copy_from_slice(&access_point);
    authentication[26..28].copy_from_slice(&[1, 0]);
    fn parse(frame: &[u8]) -> Option<ApManagementRequest<'_>> {
        parse_ap_management_request(&TEST_ADVERTISEMENT, frame, Eui48::new([2, 0, 0, 0, 0, 1]))
    }
    assert!(parse(&authentication).is_some());
    // Version (bit 0), To DS (bit 8), From DS (bit 9).
    for (octet, bit) in [(0, 0x01), (1, 0x01), (1, 0x02)] {
        let mut refused = authentication;
        refused[octet] ^= bit;
        assert_eq!(parse(&refused), None, "octet {octet} bit {bit:#04x}");
    }

    let mut probe = std::vec![0_u8; 24];
    probe[..2].copy_from_slice(&[0x40, 0x00]);
    probe[4..10].copy_from_slice(&[0xff; 6]);
    probe[10..16].copy_from_slice(&peer);
    probe[16..22].copy_from_slice(&[0xff; 6]);
    probe.extend_from_slice(&[0, 0, 1, 1, 0x82]);
    assert!(matches!(
        parse(&probe),
        Some(ApManagementRequest::Probe { .. })
    ));
    // Version, To DS, From DS, More Fragments, Protected; a fragment
    // number in Sequence Control.
    for (octet, bit) in [
        (0, 0x01),
        (1, 0x01),
        (1, 0x02),
        (1, 0x04),
        (1, 0x40),
        (22, 0x01),
    ] {
        let mut refused = probe.clone();
        refused[octet] ^= bit;
        assert_eq!(parse(&refused), None, "octet {octet} bit {bit:#04x}");
    }
}

/// A robust management subtype is read from a version-0 management frame
/// with neither DS bit, each refused bit alone refusing it.
#[test]
fn a_robust_subtype_is_read_only_from_its_frame_control() {
    let mut frame = [0_u8; AP_PEER_DISCONNECT_LEN];
    write_ap_peer_disconnect(
        &mut frame,
        Eui48::new([2, 0, 0, 0, 0, 1]),
        Eui48::new([2, 0, 0, 0, 0, 2]),
        ApPeerDisconnectKind::Deauthentication,
        crate::status::ReasonCode::new(2),
        seq(1),
    )
    .unwrap();
    assert_eq!(
        ApManagementSubtype::parse(&frame),
        Some(ApManagementSubtype::Deauthentication)
    );
    frame[0] = 0xa0;
    assert_eq!(
        ApManagementSubtype::parse(&frame),
        Some(ApManagementSubtype::Disassociation)
    );
    frame[0] = 0xd0;
    assert_eq!(
        ApManagementSubtype::parse(&frame),
        Some(ApManagementSubtype::Action)
    );
    // Version, a data frame's type bit, To DS, From DS.
    for (octet, bit) in [(0, 0x01), (0, 0x08), (1, 0x01), (1, 0x02)] {
        let mut refused = frame;
        refused[octet] ^= bit;
        assert_eq!(
            ApManagementSubtype::parse(&refused),
            None,
            "octet {octet} bit {bit:#04x}"
        );
    }
}

/// A PS-Poll needs both marker bits of its AID field and a nonzero AID.
#[test]
fn a_ps_poll_needs_its_marker_and_a_nonzero_association_id() {
    let mut frame = [0_u8; 16];
    frame[..2].copy_from_slice(&[0xa4, 0x00]);
    frame[10..16].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    for (aid, observed) in [
        ([0x23, 0xc1], true),
        ([0x23, 0x41], false),
        ([0x00, 0xc0], false),
    ] {
        frame[2..4].copy_from_slice(&aid);
        assert_eq!(
            observe_ap_power_save(&frame).is_some(),
            observed,
            "{aid:02x?}"
        );
    }
}

/// The largest association ID is written whole
/// with both marker bits, and read whole from a PS-Poll (IEEE Std
/// 802.11-2020, 9.4.1.8).
#[test]
fn the_largest_association_id_is_written_and_read_whole() {
    let mut body = [0; AP_ASSOCIATION_RESPONSE_BODY_LEN];
    write_ht_association_response(
        &TEST_ADVERTISEMENT,
        &mut body,
        crate::status::StatusCode::SUCCESS,
        crate::management::AssociationId::new(2007),
        crate::channel::Channel::ghz2_4(6, crate::channel::ChannelWidth::Mhz20).unwrap(),
        None,
        ApBssProtection::default(),
    )
    .unwrap();
    // Capability Information, Status Code, then the AID field.
    assert_eq!(&body[4..6], &[0xd7, 0xc7]);

    let peer = [1, 2, 3, 4, 5, 6];
    let mut frame = [0_u8; 16];
    frame[..2].copy_from_slice(&[0xa4, 0x00]);
    frame[2..4].copy_from_slice(&body[4..6]);
    frame[10..16].copy_from_slice(&peer);
    assert_eq!(
        observe_ap_power_save(&frame),
        Some(ApPowerSaveObservation::PsPoll {
            peer: Eui48::new(peer),
            association_id: crate::management::AssociationId::new(2007).unwrap().field()
        })
    );
}

/// A Null data frame names the access point as Receiver Address and BSSID;
/// either one of another address refuses it alone.
#[test]
fn null_data_names_the_access_point_in_both_addresses() {
    const ACCESS_POINT: [u8; 6] = [2, 0, 0, 0, 0, 1];
    let mut frame = [0_u8; STA_NULL_DATA_FRAME_LEN];
    // Null, To DS, Power Management (IEEE Std 802.11-2020, 9.2.4.1).
    frame[..2].copy_from_slice(&[0x48, 0x11]);
    frame[4..10].copy_from_slice(&ACCESS_POINT);
    frame[10..16].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
    frame[16..22].copy_from_slice(&ACCESS_POINT);
    assert!(
        observe_ap_null_data_power_save_for_access_point(&frame, Eui48::new(ACCESS_POINT))
            .is_some()
    );
    for address in [4, 16] {
        let mut other = frame;
        other[address + 5] = 9;
        assert!(
            observe_ap_null_data_power_save_for_access_point(&other, Eui48::new(ACCESS_POINT))
                .is_none(),
            "address at {address}"
        );
    }
}

#[test]
fn data_goes_only_from_a_nonzero_individual_access_point_to_such_a_peer() {
    let peer = [2, 0, 0, 0, 0, 2];
    let access_point = [2, 0, 0, 0, 0, 1];
    let frames = [msdu(peer, [0; 6], 0, &[])];
    let amsdu = |access_point, peer| {
        ApAmsduFrame {
            access_point: Eui48::new(access_point),
            peer: Eui48::new(peer),
            sequence_number: seq(0),
            user_priority: 0,
            more_data: crate::data::MoreData::Clear,
            ccmp_header: None,
            msdus: &frames,
        }
        .encode(&mut [0; 128])
    };
    let eapol = |access_point| {
        ApDataFrame {
            access_point: Eui48::new(access_point),
            destination: Eui48::new(peer),
            sequence_number: seq(0),
            ether_type: EtherType::new(0x888e),
            payload: &[1],
        }
        .encode(&mut [0; 128])
    };
    assert!(amsdu(access_point, peer).is_ok());
    assert!(eapol(access_point).is_ok());
    for refused in [[0; 6], [3, 0, 0, 0, 0, 1], [0xff; 6]] {
        assert_eq!(
            amsdu(refused, peer),
            Err(ApDataFrameError::InvalidAccessPoint)
        );
        assert_eq!(
            amsdu(access_point, refused),
            Err(ApDataFrameError::InvalidPeer)
        );
        assert_eq!(eapol(refused), Err(ApDataFrameError::InvalidAccessPoint));
    }
}

#[test]
fn a_management_request_names_the_access_point_in_both_addresses() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut frame = [0_u8; 30];
    frame[..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&access_point);
    frame[10..16].copy_from_slice(&peer);
    frame[16..22].copy_from_slice(&access_point);
    frame[26..28].copy_from_slice(&1_u16.to_le_bytes());
    let parsed = |frame: &[u8]| {
        parse_ap_management_request(&TEST_ADVERTISEMENT, frame, Eui48::new(access_point)).is_some()
    };
    assert!(parsed(&frame));
    for other in [4, 16] {
        let mut frame = frame;
        frame[other + 5] = 9;
        assert!(!parsed(&frame), "address at {other}");
    }
    for refused in [[0; 6], [3, 0, 0, 0, 0, 2]] {
        let mut frame = frame;
        frame[10..16].copy_from_slice(&refused);
        assert!(!parsed(&frame), "peer {refused:?}");
    }
}

#[test]
fn null_data_of_no_station_moves_no_power_state() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let mut frame = [0_u8; STA_NULL_DATA_FRAME_LEN];
    frame[..2].copy_from_slice(&[0x48, 0x11]);
    frame[4..10].copy_from_slice(&access_point);
    frame[16..22].copy_from_slice(&access_point);
    for (peer, observed) in [
        ([2, 0, 0, 0, 0, 2], true),
        ([0; 6], false),
        ([3, 0, 0, 0, 0, 2], false),
        ([0xff; 6], false),
    ] {
        frame[10..16].copy_from_slice(&peer);
        assert_eq!(
            observe_ap_null_data_power_save_for_access_point(&frame, Eui48::new(access_point))
                .is_some(),
            observed,
            "peer {peer:?}"
        );
    }
}

/// An Open System request is its exchange's first frame and carries success:
/// either refuses it alone.
#[test]
fn an_open_authentication_request_is_the_first_frame_with_success() {
    let mut frame = [0_u8; 30];
    frame[0] = 0xb0;
    frame[4..10].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
    frame[10..16].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
    frame[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
    for (transaction, status, read) in [(1, 0, true), (2, 0, false), (1, 1, false)] {
        frame[26] = transaction;
        frame[28] = status;
        assert_eq!(
            parse_ap_management_request(
                &TEST_ADVERTISEMENT,
                &frame,
                Eui48::new([2, 0, 0, 0, 0, 1])
            )
            .is_some(),
            read
        );
    }
}

/// An Action frame is robust by its category, and without one is not; a
/// disconnect is robust whatever it carries.
#[test]
fn an_action_is_robust_by_its_category() {
    assert!(ApManagementSubtype::Action.is_robust(&[3]));
    assert!(!ApManagementSubtype::Action.is_robust(&[4]));
    assert!(!ApManagementSubtype::Action.is_robust(&[]));
    assert!(ApManagementSubtype::Disassociation.is_robust(&[]));
    assert!(ApManagementSubtype::Deauthentication.is_robust(&[]));
}

/// A WMM element is a vendor element of at least six octets whose first four
/// are the Microsoft OUI and type 2; the legacy WPA element's are type 1.
#[test]
fn wmm_and_legacy_wpa_are_vendor_elements_of_their_type() {
    let wpa = |bytes: &[u8]| {
        association_security_observation(
            bytes,
            crate::management::CapabilityInformation::from_bytes([0; 2]),
        )
        .legacy_wpa
            == crate::scan::ElementPresence::Present
    };
    for (bytes, is_wmm, is_wpa) in [
        (&[221, 6, 0, 0x50, 0xf2, 2, 0, 1][..], true, false),
        (&[221, 5, 0, 0x50, 0xf2, 2, 0], false, false),
        (&[220, 6, 0, 0x50, 0xf2, 2, 0, 1], false, false),
        (&[221, 6, 0, 0x50, 0xf3, 2, 0, 1], false, false),
        (&[221, 4, 0, 0x50, 0xf2, 1], false, true),
        (&[220, 4, 0, 0x50, 0xf2, 1], false, false),
        (&[221, 4, 0, 0x50, 0xf3, 1], false, false),
    ] {
        assert_eq!(supports_wmm(bytes), is_wmm, "{bytes:?}");
        assert_eq!(wpa(bytes), is_wpa, "{bytes:?}");
    }
}
