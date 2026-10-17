use crate::ap::profile::tests::TEST_ADVERTISEMENT;

const TEST_HT_CAPABILITIES: crate::ht::HtLocalCapabilities =
    crate::ht::HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01);

use super::*;
use crate::protection::ApBssProtection;
use crate::sequence::seq;

#[test]
fn ap_eapol_data_frame_uses_from_ds_address_mapping() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut output = [0; 64];
    let len = ApDataFrame {
        access_point,
        destination: peer,
        sequence_number: seq(9),
        ether_type: 0x888e,
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
    let mut ethernet = [0_u8; 18];
    ethernet[..6].copy_from_slice(&peer);
    ethernet[6..12].copy_from_slice(&source);
    ethernet[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
    ethernet[14..].copy_from_slice(&[1, 2, 3, 4]);
    let ccmp = [3, 0, 0, 0x20, 0, 0, 0, 0];
    let mut output = [0; 96];
    let len = ApProtectedDataFrame {
        access_point,
        peer,
        sequence_number: seq(7),
        user_priority: 0,
        peer_qos: true,
        more_data: false,
        ccmp_header: ccmp,
        ethernet: &ethernet,
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

#[test]
fn ap_amsdu_encodes_multiple_open_and_ccmp_subframes_in_order() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut first = [0_u8; 17];
    first[..6].copy_from_slice(&peer);
    first[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 3]);
    first[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
    first[14..].copy_from_slice(&[1, 2, 3]);
    let mut second = [0_u8; 18];
    second[..6].copy_from_slice(&peer);
    second[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 4]);
    second[12..14].copy_from_slice(&0x0806_u16.to_be_bytes());
    second[14..].copy_from_slice(&[4, 5, 6, 7]);
    let mut third = [0_u8; 16];
    third[..6].copy_from_slice(&peer);
    third[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 5]);
    third[12..14].copy_from_slice(&0x86dd_u16.to_be_bytes());
    third[14..].copy_from_slice(&[8, 9]);
    let frames: [&[u8]; 3] = [&first, &second, &third];
    let ccmp = [9, 0, 0, 0x20, 0, 0, 0, 0];
    let mut protected = [0xa5; 192];
    let protected_len = ApAmsduFrame {
        access_point,
        peer,
        sequence_number: seq(11),
        user_priority: 5,
        more_data: true,
        ccmp_header: Some(ccmp),
        ethernet_frames: &frames,
    }
    .encode(&mut protected)
    .unwrap();
    assert_eq!(protected_len, ap_amsdu_frame_length(&frames, true).unwrap());
    assert_eq!(&protected[..2], &0x6288_u16.to_le_bytes());
    assert_eq!(&protected[4..10], &peer);
    assert_eq!(&protected[22..24], &0x00b0_u16.to_le_bytes());
    assert_eq!(protected[24], 0x85);
    assert_eq!(&protected[26..34], &ccmp);
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
        access_point,
        peer,
        sequence_number: seq(12),
        user_priority: 0,
        more_data: false,
        ccmp_header: None,
        ethernet_frames: &frames,
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
    let mut first = [0_u8; 14];
    first[..6].copy_from_slice(&peer);
    let mut wrong_peer = [0_u8; 14];
    wrong_peer[..6].copy_from_slice(&[2, 0, 0, 0, 0, 9]);
    let frames: [&[u8]; 2] = [&first, &wrong_peer];
    let mut output = [0xa5; 80];
    assert_eq!(
        ApAmsduFrame {
            access_point,
            peer,
            sequence_number: seq(0),
            user_priority: 0,
            more_data: false,
            ccmp_header: Some([0; 8]),
            ethernet_frames: &frames,
        }
        .encode(&mut output),
        Err(ApDataFrameError::InvalidPeer)
    );
    assert_eq!(output, [0xa5; 80]);

    let frames: [&[u8]; 2] = [&first, &first];
    let required = ap_amsdu_frame_length(&frames, true).unwrap();
    let mut short = [0x5a; 64];
    assert!(short.len() < required);
    assert_eq!(
        ApAmsduFrame {
            access_point,
            peer,
            sequence_number: seq(0),
            user_priority: 0,
            more_data: false,
            ccmp_header: Some([0; 8]),
            ethernet_frames: &frames,
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
    let mut ethernet = [0_u8; 14];
    ethernet[..6].copy_from_slice(&[2, 0, 0, 0, 0, 9]);
    let mut output = [0; 64];
    assert_eq!(
        ApProtectedDataFrame {
            access_point,
            peer,
            sequence_number: seq(0),
            user_priority: 0,
            peer_qos: false,
            more_data: false,
            ccmp_header: [0; 8],
            ethernet: &ethernet,
        }
        .encode(&mut output),
        Err(ApDataFrameError::InvalidPeer)
    );
}

#[test]
fn protected_ap_qos_frame_encodes_in_network_headroom_without_payload_copy() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let source = [2, 0, 0, 0, 0, 3];
    let ethernet_offset = 40;
    let mut storage = [0_u8; 96];
    storage[ethernet_offset..ethernet_offset + 6].copy_from_slice(&peer);
    storage[ethernet_offset + 6..ethernet_offset + 12].copy_from_slice(&source);
    storage[ethernet_offset + 12..ethernet_offset + 14].copy_from_slice(&0x0800_u16.to_be_bytes());
    storage[ethernet_offset + 14..ethernet_offset + 18].copy_from_slice(&[1, 2, 3, 4]);
    let encoded = ApProtectedDataFrame {
        access_point,
        peer,
        sequence_number: seq(7),
        user_priority: 0,
        peer_qos: true,
        more_data: false,
        ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
        ethernet: &[],
    }
    .encode_in_place(&mut storage, ethernet_offset, 18)
    .unwrap();
    assert_eq!(
        encoded.offset,
        ethernet_offset - AP_PROTECTED_QOS_ETHERNET_OVERHEAD
    );
    assert_eq!(encoded.length, 18 + AP_PROTECTED_QOS_ETHERNET_OVERHEAD);
    assert_eq!(
        &storage[encoded.offset..encoded.offset + 2],
        &0x4288_u16.to_le_bytes()
    );
    assert_eq!(
        &storage[ethernet_offset + 14..ethernet_offset + 18],
        &[1, 2, 3, 4]
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
        access_point,
        peer,
        sequence_number: seq(9),
        body: &body,
    }
    .encode(&mut frame)
    .unwrap();
    // Reverse direction for the peer-originated response parsed by AP.
    frame[4..10].copy_from_slice(&access_point);
    frame[10..16].copy_from_slice(&peer);
    assert!(matches!(
        parse_ap_management_request(&TEST_ADVERTISEMENT, &frame[..length], access_point),
        Some(ApManagementRequest::BlockAck {
            peer: parsed_peer,
            action: BlockAckAction::AddbaResponse {
                dialog_token: 7,
                tid: 0,
                window: 32,
                ..
            },
        }) if parsed_peer == peer
    ));
}

#[test]
fn association_response_owns_status_aid_and_ht_channel_capability() {
    let mut body = [0; AP_ASSOCIATION_RESPONSE_BODY_LEN];
    let ht20 = crate::channel::Channel::ghz2_4(6, crate::channel::ChannelWidth::Mhz20).unwrap();
    write_ht_association_response(
        &TEST_ADVERTISEMENT,
        &mut body,
        17,
        0x0123,
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
        0,
        1,
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
        0,
        1,
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
            0,
            0,
            ht20,
            None,
            ApBssProtection::default(),
        ),
        Err(ApAssociationResponseError::MissingAssociationId)
    );

    let ht40 =
        crate::channel::Channel::ghz2_4(6, crate::channel::WifiChannelWidth::Mhz40Below).unwrap();
    write_ht_association_response(
        &TEST_ADVERTISEMENT,
        &mut body,
        0,
        1,
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
            access_point,
            peer,
            ApPeerDisconnectKind::Disassociation,
            4,
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
        access_point,
        peer,
        ApPeerDisconnectKind::Deauthentication,
        2,
        seq(8),
    )
    .unwrap();
    assert_eq!(&output[..2], &0x00c0_u16.to_le_bytes());
    assert_eq!(&output[24..26], &2_u16.to_le_bytes());
}

#[test]
fn tim_bitmap_update_matches_aid_bit_selection() {
    use crate::beacon::{TimAssociationId, TimVirtualBitmap};

    let mut bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
    bitmap.set(TimAssociationId::new(7).unwrap(), true).unwrap();
    bitmap.set(TimAssociationId::new(8).unwrap(), true).unwrap();
    bitmap
        .set(TimAssociationId::new(15).unwrap(), true)
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
        Some(ApPowerSaveObservation::Sleeping { peer })
    );
    frame[..2].copy_from_slice(&0x0108_u16.to_le_bytes());
    assert_eq!(
        observe_ap_power_save(&frame),
        Some(ApPowerSaveObservation::Active { peer })
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
            peer,
            association_id: 0x123
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
            crate::address::IndividualAddress::new(peer).unwrap(),
        ),
        bssid: crate::address::Bssid::new(
            crate::address::IndividualAddress::new(access_point).unwrap(),
        )
        .unwrap(),
        sequence_number: seq(1),
        transaction: 1,
        status_code: 126,
        body: &commit,
    }
    .encode(&mut request)
    .unwrap();
    assert_eq!(
        parse_ap_management_request(&TEST_ADVERTISEMENT, &request[..len], access_point),
        Some(ApManagementRequest::SaeAuthentication {
            peer,
            transaction: 1,
            status: 126,
            body: &commit,
        })
    );

    let confirm = [1, 0, 0xcc];
    let mut response = [0_u8; 64];
    let len = write_sae_authentication(&mut response, access_point, peer, 2, 0, &confirm, seq(2))
        .unwrap();
    let parsed = parse_sae_authentication(&response[..len], peer, access_point).unwrap();
    assert_eq!(parsed.transaction, 2);
    assert_eq!(parsed.status_code, 0);
    assert_eq!(parsed.body, confirm);
    assert_eq!(
        write_sae_authentication(
            &mut response[..32],
            access_point,
            peer,
            2,
            0,
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
        parse_ap_management_request(&TEST_ADVERTISEMENT, &authentication, access_point),
        Some(ApManagementRequest::OpenAuthentication { peer })
    );
    authentication[4] ^= 1;
    assert_eq!(
        parse_ap_management_request(&TEST_ADVERTISEMENT, &authentication, access_point),
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
        parse_ap_management_request(&TEST_ADVERTISEMENT, &association, access_point),
        Some(ApManagementRequest::Association {
            peer,
            security: ApAssociationSecurityObservation {
                privacy: false,
                rsn_ie: Some(&association[39..42]),
                rsn_ie_count: 1,
                rsnxe: None,
                rsnxe_count: 0,
                legacy_wpa_present: false,
                malformed_elements: false,
            },
            maximum_legacy_rate_500kbps: 108,
            ht_capabilities: None,
            qos_supported: false,
            short_preamble: false,
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
        parse_ap_management_request(&TEST_ADVERTISEMENT, &association, access_point)
    else {
        panic!("association request must parse");
    };
    assert_eq!(security.rsn_ie, Some(&association[28..31]));
    assert_eq!(security.rsn_ie_count, 1);
    assert_eq!(security.rsnxe, Some(&association[31..34]));
    assert_eq!(security.rsnxe_count, 2);
    assert!(!security.malformed_elements);
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
        crate::channel::Channel::ghz2_4(6, crate::channel::WifiChannelWidth::Mhz40Above).unwrap();
    association[34..].copy_from_slice(&crate::ht::ht_capability_ie(TEST_HT_CAPABILITIES, channel));

    let Some(ApManagementRequest::Association {
        maximum_legacy_rate_500kbps,
        ht_capabilities: Some(ht),
        qos_supported,
        ..
    }) = parse_ap_management_request(&TEST_ADVERTISEMENT, &association, access_point)
    else {
        panic!("complete HT40 association request must parse");
    };
    assert_eq!(maximum_legacy_rate_500kbps, 108);
    assert!(ht.supports_40_mhz());
    assert!(ht.supports_short_guard_interval(crate::channel::WifiChannelWidth::Mhz40Above));
    assert_eq!(ht.highest_rx_mcs(), 7);
    assert!(qos_supported);
}

#[test]
fn complete_response_encoders_own_addresses_sequence_and_status() {
    let access_point = [2, 0, 0, 0, 0, 1];
    let peer = [2, 0, 0, 0, 0, 2];
    let mut authentication = [0xaa; AP_AUTHENTICATION_RESPONSE_LEN];
    write_open_authentication_response(&mut authentication, access_point, peer, 17, seq(7))
        .unwrap();
    assert_eq!(&authentication[4..10], &peer);
    assert_eq!(&authentication[10..16], &access_point);
    assert_eq!(&authentication[26..28], &2_u16.to_le_bytes());
    assert_eq!(&authentication[28..30], &17_u16.to_le_bytes());

    let mut association = [0; AP_ASSOCIATION_RESPONSE_LEN];
    write_ht_association_response_frame_for_security(
        &TEST_ADVERTISEMENT,
        &mut association,
        access_point,
        peer,
        0,
        0xc001,
        seq(8),
        crate::channel::Channel::ghz2_4(6, crate::channel::ChannelWidth::Mhz20).unwrap(),
        None,
        ApSecurityPolicy::Wpa2Personal,
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
            0,
            1,
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
        transaction: [0x12, 0x34],
    }
    .encode();
    let mut frame = [0; 64];
    let length = ApProtectedManagementFrame {
        subtype: ApManagementSubtype::Action,
        access_point: ap,
        peer,
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
        parse_ap_management_request(&TEST_ADVERTISEMENT, &frame[..length], ap),
        Some(ApManagementRequest::SaQuery {
            peer,
            query: SaQuery::Request {
                transaction: [0x12, 0x34]
            }
        })
    );
    for subtype in [
        ApManagementSubtype::Deauthentication,
        ApManagementSubtype::Disassociation,
    ] {
        let length = ApProtectedManagementFrame {
            subtype,
            access_point: ap,
            peer,
            sequence_number: seq(10),
            ccmp_header: ccmp,
            body: &[3, 0],
        }
        .encode(&mut frame)
        .unwrap();
        frame[4..10].copy_from_slice(&ap);
        frame[10..16].copy_from_slice(&peer);
        assert!(matches!(
            parse_ap_management_request(&TEST_ADVERTISEMENT, &frame[..length], ap),
            Some(
                ApManagementRequest::Deauthentication { reason: 3, .. }
                    | ApManagementRequest::Disassociation { reason: 3, .. }
            )
        ));
        assert!(parse_ap_management_request(&TEST_ADVERTISEMENT, &frame[..31], ap).is_none());
    }
}
