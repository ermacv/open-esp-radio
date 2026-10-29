use super::*;
use crate::security::{AssociationAkm, AssociationSecurity, Pmkid, SaePwe, Spp};
use crate::sequence::seq;
use oer_network::ETHERNET_HEADER_LEN;

const LOCAL: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];
const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];

fn authentication_response(status_code: u16) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&OPEN_AUTHENTICATION_FRAME_CONTROL.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[26..28].copy_from_slice(&OPEN_SYSTEM_RESPONSE_SEQUENCE.to_le_bytes());
    frame[28..30].copy_from_slice(&status_code.to_le_bytes());
    frame
}

#[test]
fn encodes_open_authentication_request() {
    let mut output = [0xa5; 32];
    let length = OpenAuthenticationRequest {
        source: crate::address::OwnAddress::new(
            crate::address::IndividualAddress::new(LOCAL).unwrap(),
        ),
        bssid: crate::address::Bssid::new(crate::address::IndividualAddress::new(BSSID).unwrap())
            .unwrap(),
        sequence_number: seq(0x123),
    }
    .encode(&mut output)
    .unwrap();
    assert_eq!(length, 30);
    assert_eq!(&output[0..2], &[0xb0, 0]);
    assert_eq!(&output[4..10], &BSSID);
    assert_eq!(&output[10..16], &LOCAL);
    assert_eq!(&output[16..22], &BSSID);
    assert_eq!(&output[22..24], &[0x30, 0x12]);
    assert_eq!(&output[24..30], &[0, 0, 1, 0, 0, 0]);
    assert_eq!(output[30], 0xa5);
}

#[test]
fn encodes_sta_action_frame_around_owned_body() {
    let body = [3, 1, 7, 0, 0, 0x02, 0x04, 0, 0];
    let mut output = [0xa5; 40];
    let length = StaManagementFrame {
        subtype: StaManagementSubtype::Action,
        source: crate::address::OwnAddress::new(
            crate::address::IndividualAddress::new(LOCAL).unwrap(),
        ),
        bssid: crate::address::Bssid::new(crate::address::IndividualAddress::new(BSSID).unwrap())
            .unwrap(),
        sequence_number: seq(0x123),
        body: &body,
    }
    .encode(&mut output)
    .unwrap();
    assert_eq!(length, 33);
    assert_eq!(&output[0..2], &[0xd0, 0]);
    assert_eq!(&output[4..10], &BSSID);
    assert_eq!(&output[10..16], &LOCAL);
    assert_eq!(&output[16..22], &BSSID);
    assert_eq!(&output[22..24], &[0x30, 0x12]);
    assert_eq!(&output[24..33], &body);
    assert_eq!(output[33], 0xa5);
}

#[test]
fn encodes_protected_action_frame_with_its_ccmp_header() {
    let body = [8, 1, 0x12, 0x34];
    let ccmp_header = [5, 0, 0, 0x20, 0, 0, 0, 0];
    let mut output = [0xa5; 40];
    let length = StaProtectedManagementFrame {
        subtype: StaManagementSubtype::Action,
        source: crate::address::OwnAddress::new(
            crate::address::IndividualAddress::new(LOCAL).unwrap(),
        ),
        bssid: crate::address::Bssid::new(crate::address::IndividualAddress::new(BSSID).unwrap())
            .unwrap(),
        sequence_number: seq(0x123),
        ccmp_header,
        body: &body,
    }
    .encode(&mut output)
    .unwrap();
    // Hardware appends the MIC after the body.
    assert_eq!(length, 36);
    assert_eq!(&output[0..2], &[0xd0, 0x40]);
    assert_eq!(&output[4..10], &BSSID);
    assert_eq!(&output[10..16], &LOCAL);
    assert_eq!(&output[22..24], &[0x30, 0x12]);
    assert_eq!(&output[24..32], &ccmp_header);
    assert_eq!(&output[32..36], &body);
    assert_eq!(output[36], 0xa5);
}

#[test]
fn a_deauthentication_carries_its_reason_plain_or_protected() {
    let reason = 3_u16.to_le_bytes();
    let mut output = [0xa5; 40];
    let length = StaManagementFrame {
        subtype: StaManagementSubtype::Deauthentication,
        source: crate::address::OwnAddress::new(
            crate::address::IndividualAddress::new(LOCAL).unwrap(),
        ),
        bssid: crate::address::Bssid::new(crate::address::IndividualAddress::new(BSSID).unwrap())
            .unwrap(),
        sequence_number: seq(0x10),
        body: &reason,
    }
    .encode(&mut output)
    .unwrap();
    assert_eq!(length, 26);
    assert_eq!(
        parse_sta_disconnect(
            &output[..length],
            crate::address::OwnAddress::new(crate::address::IndividualAddress::new(LOCAL).unwrap()),
            crate::address::Bssid::new(crate::address::IndividualAddress::new(BSSID).unwrap())
                .unwrap(),
        ),
        None,
        "the station's own Deauthentication is not addressed to it"
    );
    assert_eq!(&output[0..2], &[0xc0, 0]);
    assert_eq!(&output[4..10], &BSSID);
    assert_eq!(&output[24..26], &reason);

    let ccmp_header = [5, 0, 0, 0x20, 0, 0, 0, 0];
    let length = StaProtectedManagementFrame {
        subtype: StaManagementSubtype::Deauthentication,
        source: crate::address::OwnAddress::new(
            crate::address::IndividualAddress::new(LOCAL).unwrap(),
        ),
        bssid: crate::address::Bssid::new(crate::address::IndividualAddress::new(BSSID).unwrap())
            .unwrap(),
        sequence_number: seq(0x11),
        ccmp_header,
        body: &reason,
    }
    .encode(&mut output)
    .unwrap();
    assert_eq!(length, 34);
    assert_eq!(&output[0..2], &[0xc0, 0x40]);
    assert_eq!(&output[24..32], &ccmp_header);
    assert_eq!(&output[32..34], &reason);
}

#[test]
fn sae_authentication_frames_round_trip() {
    let body = [0x13, 0x00, 0xaa, 0xbb];
    let mut output = [0xa5; 40];
    let length = SaeAuthenticationFrame {
        source: crate::address::OwnAddress::new(
            crate::address::IndividualAddress::new(LOCAL).unwrap(),
        ),
        bssid: crate::address::Bssid::new(crate::address::IndividualAddress::new(BSSID).unwrap())
            .unwrap(),
        sequence_number: seq(0x123),
        transaction: SAE_COMMIT_TRANSACTION,
        status_code: 126,
        body: &body,
    }
    .encode(&mut output)
    .unwrap();
    assert_eq!(length, 34);
    assert_eq!(&output[0..2], &[0xb0, 0]);
    assert_eq!(&output[24..30], &[3, 0, 1, 0, 126, 0]);
    assert_eq!(&output[30..34], &body);

    // The access point's reply swaps the addresses.
    let mut reply = output;
    reply[4..10].copy_from_slice(&LOCAL);
    reply[10..16].copy_from_slice(&BSSID);
    reply[26] = 2;
    assert_eq!(
        parse_sae_authentication(&reply[..length], LOCAL, BSSID),
        Some(SaeAuthentication {
            transaction: SAE_CONFIRM_TRANSACTION,
            status_code: 126,
            body: &body,
        })
    );
    reply[24] = 0;
    assert_eq!(
        parse_sae_authentication(&reply[..length], LOCAL, BSSID),
        None
    );
}

#[test]
fn parses_only_matching_open_authentication_response() {
    let frame = authentication_response(17);
    assert_eq!(
        parse_open_authentication_response(&frame, LOCAL, BSSID),
        Some(OpenAuthenticationResponse { status_code: 17 })
    );
    assert_eq!(
        parse_open_authentication_response(&frame, [0; 6], BSSID),
        None
    );
}

#[test]
fn disconnect_parser_checks_receiver_bss_geometry_and_protection() {
    use crate::address::{Bssid, IndividualAddress, OwnAddress};
    let local = OwnAddress::new(IndividualAddress::new(LOCAL).unwrap());
    let bssid = Bssid::new(IndividualAddress::new(BSSID).unwrap()).unwrap();
    for (control, kind) in [
        (
            DEAUTHENTICATION_FRAME_CONTROL,
            StaDisconnectKind::Deauthentication,
        ),
        (
            DISASSOCIATION_FRAME_CONTROL,
            StaDisconnectKind::Disassociation,
        ),
    ] {
        let mut frame = [0_u8; MANAGEMENT_HEADER_LEN + 2];
        frame[..2].copy_from_slice(&control.to_le_bytes());
        frame[4..10].copy_from_slice(&LOCAL);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[24..26].copy_from_slice(&7_u16.to_le_bytes());
        let expected = Some(StaDisconnect {
            kind,
            reason_code: 7,
        });
        assert_eq!(parse_sta_disconnect(&frame, local, bssid), expected);
        let mut broadcast = frame;
        broadcast[4..10].fill(0xff);
        assert_eq!(parse_sta_disconnect(&broadcast, local, bssid), expected);
        for (offset, value) in [(4, 4), (4, 1), (10, 4), (16, 4), (22, 1)] {
            let mut foreign = frame;
            foreign[offset] = value;
            assert_eq!(parse_sta_disconnect(&foreign, local, bssid), None);
        }
        for bits in [1, 4, 8, 0x100, 0x200, 0x400, 0x4000] {
            let mut invalid = frame;
            invalid[..2].copy_from_slice(&(control | bits).to_le_bytes());
            assert_eq!(parse_sta_disconnect(&invalid, local, bssid), None);
        }
        for length in 0..frame.len() {
            assert_eq!(parse_sta_disconnect(&frame[..length], local, bssid), None);
        }
    }
}

#[test]
fn he_ul_mu_power_rejects_a_rate_above_the_reference() {
    assert_eq!(
        HeUlMuPowerCapability::from_rate_power_indices([20, 20, 20, 19, 19, 21, 18, 16, 15, 20,]),
        Err(HeUlMuPowerCapabilityError::HigherPowerThanRate16 { rate: 21 })
    );
}

#[test]
fn sta_data_frame_encodes_to_ds_llc_snap() {
    let mut output = [0; 64];
    let len = StaDataFrame {
        source: crate::address::OwnAddress::new(
            crate::address::IndividualAddress::new([2, 3, 4, 5, 6, 7]).unwrap(),
        ),
        bssid: crate::address::Bssid::new(
            crate::address::IndividualAddress::new([0x10, 0x11, 0x12, 0x13, 0x14, 0x15]).unwrap(),
        )
        .unwrap(),
        destination: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        sequence_number: seq(0x123),
        ether_type: 0x888e,
        payload: &[1, 2, 3],
    }
    .encode(&mut output)
    .unwrap();
    assert_eq!(len, 35);
    assert_eq!(&output[..2], &[0x08, 0x01]);
    assert_eq!(&output[4..10], &[0x10, 0x11, 0x12, 0x13, 0x14, 0x15]);
    assert_eq!(&output[10..16], &[2, 3, 4, 5, 6, 7]);
    assert_eq!(&output[16..22], &[0x20, 0x21, 0x22, 0x23, 0x24, 0x25]);
    assert_eq!(&output[24..32], &[0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e]);
    assert_eq!(&output[32..len], &[1, 2, 3]);
}

#[test]
fn protected_data_frame_selects_the_recovered_legacy_or_qos_layout() {
    let mut output = [0; 64];
    let frame = StaProtectedDataFrame {
        source: crate::address::OwnAddress::new(
            crate::address::IndividualAddress::new([2, 3, 4, 5, 6, 7]).unwrap(),
        ),
        bssid: crate::address::Bssid::new(
            crate::address::IndividualAddress::new([0x10, 0x11, 0x12, 0x13, 0x14, 0x15]).unwrap(),
        )
        .unwrap(),
        destination: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        sequence_number: seq(0x123),
        user_priority: 7,
        peer_qos: true,
        ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
        ether_type: 0x888e,
        payload: &[1, 2, 3],
    };
    let len = frame.encode(&mut output).unwrap();
    assert_eq!(len, 45);
    assert_eq!(&output[..2], &[0x88, 0x41]);
    assert_eq!(&output[24..26], &[7, 0]);
    assert_eq!(&output[26..34], &[3, 0, 0, 0x20, 0, 0, 0, 0]);
    assert_eq!(&output[34..42], &[0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e]);
    assert_eq!(&output[42..len], &[1, 2, 3]);

    let len = StaProtectedDataFrame {
        peer_qos: false,
        ..frame
    }
    .encode(&mut output)
    .unwrap();
    assert_eq!(len, 43);
    assert_eq!(&output[..2], &[0x08, 0x41]);
    assert_eq!(&output[24..32], &[3, 0, 0, 0x20, 0, 0, 0, 0]);
    assert_eq!(&output[32..40], &[0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e]);
    assert_eq!(&output[40..len], &[1, 2, 3]);
}

#[test]
fn protected_data_frame_fully_overwrites_a_reused_dma_slot() {
    let frame = StaProtectedDataFrame {
        source: crate::address::OwnAddress::new(
            crate::address::IndividualAddress::new([2, 3, 4, 5, 6, 7]).unwrap(),
        ),
        bssid: crate::address::Bssid::new(
            crate::address::IndividualAddress::new([0x10, 0x11, 0x12, 0x13, 0x14, 0x15]).unwrap(),
        )
        .unwrap(),
        destination: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        sequence_number: seq(0x123),
        user_priority: 7,
        peer_qos: true,
        ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
        ether_type: 0x888e,
        payload: &[1, 2, 3],
    };
    let mut zeroed = [0_u8; 64];
    let mut reused = [0xa5_u8; 64];

    let zeroed_len = frame.encode(&mut zeroed).unwrap();
    let reused_len = frame.encode(&mut reused).unwrap();

    assert_eq!(reused_len, zeroed_len);
    assert_eq!(&reused[..reused_len], &zeroed[..zeroed_len]);
}

#[test]
fn protected_ethernet_frame_reuses_payload_at_its_final_dma_offset() {
    let ethernet = [
        0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 2, 3, 4, 5, 6, 7, 0x88, 0x8e, 1, 2, 3,
    ];
    let metadata = StaProtectedEthernetFrame {
        bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
        sequence_number: seq(0x123),
        user_priority: 7,
        peer_qos: true,
        ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
    };
    let mut expected = [0_u8; 64];
    let expected_len = StaProtectedDataFrame {
        source: crate::address::OwnAddress::new(
            crate::address::IndividualAddress::new(ethernet[6..12].try_into().unwrap()).unwrap(),
        ),
        bssid: crate::address::Bssid::new(
            crate::address::IndividualAddress::new(metadata.bssid).unwrap(),
        )
        .unwrap(),
        destination: ethernet[..6].try_into().unwrap(),
        sequence_number: metadata.sequence_number,
        user_priority: metadata.user_priority,
        peer_qos: metadata.peer_qos,
        ccmp_header: metadata.ccmp_header,
        ether_type: u16::from_be_bytes([ethernet[12], ethernet[13]]),
        payload: &ethernet[14..],
    }
    .encode(&mut expected)
    .unwrap();

    let mut storage = [0xa5_u8; 64];
    storage
        [STA_PROTECTED_QOS_ETHERNET_HEADROOM..STA_PROTECTED_QOS_ETHERNET_HEADROOM + ethernet.len()]
        .copy_from_slice(&ethernet);
    let encoded = metadata
        .encode_in_place(
            &mut storage,
            STA_PROTECTED_QOS_ETHERNET_HEADROOM,
            ethernet.len(),
            DataHeControl::Disabled,
        )
        .unwrap();

    assert_eq!(
        encoded,
        EncodedStaFrame {
            offset: 0,
            length: 45
        }
    );
    assert_eq!(encoded.length, expected_len);
    assert_eq!(&storage[..encoded.length], &expected[..expected_len]);
    // The three payload bytes began at offset 28 + 14 and remain at that
    // exact address after the prefix conversion.
    assert_eq!(&storage[42..45], &[1, 2, 3]);
}

#[test]
fn protected_ethernet_frame_reports_missing_headroom_before_mutation() {
    let ethernet = [
        0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 2, 3, 4, 5, 6, 7, 0x08, 0x00,
    ];
    let mut storage = [0xa5_u8; 64];
    storage[27..27 + ethernet.len()].copy_from_slice(&ethernet);

    assert_eq!(
        StaProtectedEthernetFrame {
            bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
            sequence_number: seq(0x123),
            user_priority: 0,
            peer_qos: true,
            ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
        }
        .encode_in_place(&mut storage, 27, ethernet.len(), DataHeControl::Disabled),
        Err(StationFrameError::EthernetHeadroomTooSmall {
            required: STA_PROTECTED_QOS_ETHERNET_HEADROOM,
            available: 27,
        })
    );
    assert_eq!(&storage[27..27 + ethernet.len()], &ethernet);
}

#[test]
fn protected_he_control_keeps_ccmp_immediately_after_qos() {
    let mut output = [0xa5; 64];
    let len = StaProtectedDataFrame {
        source: crate::address::OwnAddress::new(
            crate::address::IndividualAddress::new([2, 3, 4, 5, 6, 7]).unwrap(),
        ),
        bssid: crate::address::Bssid::new(
            crate::address::IndividualAddress::new([0x10, 0x11, 0x12, 0x13, 0x14, 0x15]).unwrap(),
        )
        .unwrap(),
        destination: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        sequence_number: seq(0x123),
        user_priority: 0,
        peer_qos: true,
        ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
        ether_type: 0x0806,
        payload: &[1, 2, 3],
    }
    .encode_with_he_control(
        DataHeControl::HardwareGeneratedBufferStatusReport,
        &mut output,
    )
    .unwrap();

    assert_eq!(len, 45);
    assert_eq!(&output[..2], &[0x88, 0xc1]);
    assert_eq!(&output[26..34], &[3, 0, 0, 0x20, 0, 0, 0, 0]);
    assert_eq!(&output[34..42], &[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x06]);
    assert_eq!(&output[42..len], &[1, 2, 3]);
}

#[test]
fn protected_amsdu_encodes_two_ethernet_frames_and_round_trips() {
    let first = [
        0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 2, 3, 4, 5, 6, 7, 0x08, 0x00, 1, 2, 3,
    ];
    let second = [
        0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 8, 9, 10, 11, 12, 13, 0x88, 0xb5, 4, 5, 6,
    ];
    let ethernet_frames: [&[u8]; 2] = [&first, &second];
    let mut output = [0_u8; 96];
    assert_eq!(sta_protected_amsdu_frame_length(&ethernet_frames), Ok(87));
    let length = StaProtectedAmsduFrame {
        source: [2, 3, 4, 5, 6, 7],
        bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
        sequence_number: seq(0x123),
        user_priority: 7,
        ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
        ethernet_frames: &ethernet_frames,
    }
    .encode(&mut output)
    .unwrap();

    assert_eq!(length, 87);
    assert_eq!(&output[..2], &[0x88, 0x41]);
    assert_eq!(&output[24..26], &[0x87, 0]);
    assert_eq!(&output[26..34], &[3, 0, 0, 0x20, 0, 0, 0, 0]);
    assert_eq!(&output[34..40], &first[..6]);
    assert_eq!(&output[40..46], &first[6..12]);
    assert_eq!(&output[46..48], &[0, 11]);
    assert_eq!(&output[48..56], &[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00]);
    assert_eq!(&output[56..59], &[1, 2, 3]);
    assert_eq!(&output[59..62], &[0, 0, 0]);

    let mut subframes = crate::data::amsdu_subframes(
        DataInterfaceRole::AccessPoint,
        &output[..length],
        34,
        length - 34,
    )
    .unwrap();
    let first_decoded = subframes.next().unwrap().unwrap();
    assert_eq!(first_decoded.destination, first[..6]);
    assert_eq!(first_decoded.source, first[6..12]);
    assert_eq!(first_decoded.ether_type, 0x0800);
    assert_eq!(first_decoded.payload, &[1, 2, 3]);
    let second_decoded = subframes.next().unwrap().unwrap();
    assert_eq!(second_decoded.destination, second[..6]);
    assert_eq!(second_decoded.source, second[6..12]);
    assert_eq!(second_decoded.ether_type, 0x88b5);
    assert_eq!(second_decoded.payload, &[4, 5, 6]);
    assert_eq!(subframes.next(), None);
}

#[test]
fn protected_amsdu_pair_encodes_in_first_ethernet_allocation() {
    let first = [
        0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 2, 3, 4, 5, 6, 7, 0x08, 0x00, 1, 2, 3,
    ];
    let second = [
        0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 8, 9, 10, 11, 12, 13, 0x88, 0xb5, 4, 5, 6,
    ];
    for first_length in ETHERNET_HEADER_LEN..=first.len() {
        for second_length in ETHERNET_HEADER_LEN..=second.len() {
            let first = &first[..first_length];
            let second = &second[..second_length];
            let ethernet_frames: [&[u8]; 2] = [first, second];
            let metadata = StaProtectedEthernetFrame {
                bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
                sequence_number: seq(0x123),
                user_priority: 7,
                peer_qos: true,
                ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
            };
            let mut expected = [0_u8; 96];
            let expected_length = StaProtectedAmsduFrame {
                source: [2, 3, 4, 5, 6, 7],
                bssid: metadata.bssid,
                sequence_number: metadata.sequence_number,
                user_priority: metadata.user_priority,
                ccmp_header: metadata.ccmp_header,
                ethernet_frames: &ethernet_frames,
            }
            .encode(&mut expected)
            .unwrap();

            const ETHERNET_OFFSET: usize = STA_PROTECTED_QOS_ETHERNET_HEADROOM;
            let mut storage = [0xa5_u8; 128];
            storage[ETHERNET_OFFSET..ETHERNET_OFFSET + first.len()].copy_from_slice(first);
            let encoded = metadata
                .encode_amsdu_pair_in_place(&mut storage, ETHERNET_OFFSET, first.len(), second)
                .unwrap();

            assert_eq!(encoded.offset, 0);
            assert_eq!(encoded.length, expected_length);
            assert_eq!(
                &storage[..encoded.length],
                &expected[..expected_length],
                "in-place and owned encoders must emit identical MPDUs"
            );
            assert!(
                storage[encoded.length..].iter().all(|byte| *byte == 0xa5),
                "capacity beyond the encoded MPDU remains untouched"
            );
        }
    }
}

#[test]
fn protected_amsdu_fully_overwrites_a_reused_output_slot() {
    let first = [
        0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 2, 3, 4, 5, 6, 7, 0x08, 0x00, 1, 2, 3,
    ];
    let second = [
        0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 8, 9, 10, 11, 12, 13, 0x88, 0xb5, 4, 5, 6,
    ];
    let ethernet_frames: [&[u8]; 2] = [&first, &second];
    let encoded = StaProtectedAmsduFrame {
        source: [2, 3, 4, 5, 6, 7],
        bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
        sequence_number: seq(0x123),
        user_priority: 7,
        ccmp_header: [3, 0, 0, 0x20, 0, 0, 0, 0],
        ethernet_frames: &ethernet_frames,
    };
    let mut zeroed = [0_u8; 96];
    let mut reused = [0xa5_u8; 96];
    let zeroed_length = encoded.encode(&mut zeroed).unwrap();
    let reused_length = encoded.encode(&mut reused).unwrap();

    assert_eq!(zeroed_length, reused_length);
    assert_eq!(
        &zeroed[..zeroed_length],
        &reused[..reused_length],
        "every returned byte, including A-MSDU padding, must be initialized"
    );
    assert_eq!(&reused[59..62], &[0, 0, 0]);
    assert!(
        reused[reused_length..].iter().all(|byte| *byte == 0xa5),
        "the encoder must not touch capacity beyond the returned frame"
    );

    let previous = reused;
    let refreshed_length = StaProtectedAmsduFrame {
        sequence_number: seq(0x456),
        ccmp_header: [9, 0, 0, 0x20, 0, 0, 0, 0],
        ..encoded
    }
    .refresh_header(&mut reused)
    .unwrap();
    assert_eq!(refreshed_length, reused_length);
    assert_eq!(&reused[22..24], &(0x456_u16 << 4).to_le_bytes());
    assert_eq!(&reused[26..34], &[9, 0, 0, 0x20, 0, 0, 0, 0]);
    assert_eq!(
        &reused[34..reused_length],
        &previous[34..reused_length],
        "refresh must retain the already encoded A-MSDU body"
    );
}

#[test]
fn protected_amsdu_rejects_the_unadvertised_large_class() {
    let ethernet = [0_u8; 2_000];
    let frames: [&[u8]; 2] = [&ethernet, &ethernet];
    assert_eq!(
        sta_protected_amsdu_frame_length(&frames),
        Err(StationFrameError::AmsduTooLong {
            length: 4_016,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        })
    );
    assert_eq!(
        StaProtectedAmsduFrame {
            source: [2, 3, 4, 5, 6, 7],
            bssid: [0x10, 0x11, 0x12, 0x13, 0x14, 0x15],
            sequence_number: seq(1),
            user_priority: 0,
            ccmp_header: [0; CCMP_HEADER_LEN],
            ethernet_frames: &frames,
        }
        .encode(&mut [0_u8; 4_096]),
        Err(StationFrameError::AmsduTooLong {
            length: 4_016,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        })
    );
}

#[test]
fn parses_association_response_and_masks_aid() {
    let mut frame = [0_u8; 60];
    frame[0] = 0x10;
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..26].copy_from_slice(&0x0431_u16.to_le_bytes());
    frame[28..30].copy_from_slice(&0xc02a_u16.to_le_bytes());
    frame[30..58].copy_from_slice(&[
        45, 26, 0x20, 0, 0, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0,
    ]);
    let response = parse_association_response(&frame[..58], LOCAL, BSSID).unwrap();
    assert_eq!(response.status_code, 0);
    assert_eq!(response.association_id, 42);
    assert!(response.ht_capability);
    assert!(response.wmm);
}

#[test]
fn association_response_retains_complete_wmm_parameters() {
    let mut frame = [0_u8; 56];
    frame[0] = 0x10;
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[28..30].copy_from_slice(&7_u16.to_le_bytes());
    frame[30..].copy_from_slice(&[
        221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 3, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42,
        0x43, 94, 0, 0x62, 0x32, 47, 0,
    ]);

    let response = parse_association_response(&frame, LOCAL, BSSID).unwrap();
    let parameters = response.wmm_parameters.unwrap();
    assert_eq!(parameters.parameter_set_count, 3);
    assert_eq!(
        parameters
            .access_category(crate::qos::WmmAccessCategory::Video)
            .txop_limit_units_32_us,
        94
    );
    assert_eq!(
        parameters
            .access_category(crate::qos::WmmAccessCategory::Voice)
            .txop_limit_units_32_us,
        47
    );
}

#[test]
fn in_place_station_encoders_reject_hostile_ranges_before_writing() {
    let frame = StaProtectedEthernetFrame {
        bssid: [2, 0, 0, 0, 0, 1],
        sequence_number: seq(0),
        user_priority: 0,
        peer_qos: true,
        ccmp_header: [0; CCMP_HEADER_LEN],
    };
    for (offset, length) in [
        (usize::MAX, 14),
        (1, usize::MAX),
        (64, 14),
        (0, 13),
        (0, 14),
    ] {
        let mut storage = [0xa5; 64];
        assert!(
            frame
                .encode_in_place(&mut storage, offset, length, DataHeControl::Disabled)
                .is_err()
        );
        assert_eq!(storage, [0xa5; 64]);
        assert!(
            frame
                .encode_amsdu_pair_in_place(&mut storage, offset, length, &[0; 14])
                .is_err()
        );
        assert_eq!(storage, [0xa5; 64]);
    }
    for (offset, length) in [(usize::MAX, 14), (1, usize::MAX)] {
        let mut storage = [0xa5; 64];
        assert_eq!(
            frame.encode_in_place(&mut storage, offset, length, DataHeControl::Disabled),
            Err(StationFrameError::LengthOverflow)
        );
        assert_eq!(
            frame.encode_amsdu_pair_in_place(&mut storage, offset, length, &[0; 14]),
            Err(StationFrameError::LengthOverflow)
        );
        assert_eq!(storage, [0xa5; 64]);
    }
    let mut storage = [0xa5; 64];
    assert!(
        frame
            .encode_amsdu_pair_in_place(&mut storage, 28, 14, &[0; 13])
            .is_err()
    );
    assert_eq!(storage, [0xa5; 64]);
}

#[test]
fn station_encoders_admit_complete_capacity_before_writing() {
    use crate::address::{Bssid, IndividualAddress, OwnAddress};
    let source = OwnAddress::new(IndividualAddress::new(LOCAL).unwrap());
    let bssid = Bssid::new(IndividualAddress::new(BSSID).unwrap()).unwrap();
    let sequence_number = seq(0);
    let data = StaDataFrame {
        source,
        bssid,
        destination: BSSID,
        sequence_number,
        ether_type: 0x888e,
        payload: &[1, 2, 3],
    };
    let protected_data = StaProtectedDataFrame {
        source,
        bssid,
        destination: BSSID,
        sequence_number,
        user_priority: 7,
        peer_qos: true,
        ccmp_header: [0; CCMP_HEADER_LEN],
        ether_type: 0x0800,
        payload: &[1, 2, 3],
    };
    let open = OpenAuthenticationRequest {
        source,
        bssid,
        sequence_number,
    };
    let management = StaManagementFrame {
        source,
        bssid,
        sequence_number,
        subtype: StaManagementSubtype::Action,
        body: &[3, 1, 0],
    };
    let protected_management = StaProtectedManagementFrame {
        source,
        bssid,
        sequence_number,
        subtype: StaManagementSubtype::Action,
        ccmp_header: [0; CCMP_HEADER_LEN],
        body: &[3, 1, 0],
    };
    let sae = SaeAuthenticationFrame {
        source,
        bssid,
        sequence_number,
        transaction: 1,
        status_code: 0,
        body: &[1, 2, 3],
    };
    type Encoder<'a> = dyn Fn(&mut [u8]) -> Result<usize, StationFrameError> + 'a;
    let encoders: [&Encoder<'_>; 6] = [
        &|out| data.encode(out),
        &|out| protected_data.encode(out),
        &|out| open.encode(out),
        &|out| management.encode(out),
        &|out| protected_management.encode(out),
        &|out| sae.encode(out),
    ];
    for encode in encoders {
        let mut complete = [0xa5; 128];
        let required = encode(&mut complete).unwrap();
        for capacity in 0..required {
            let mut short = [0xa5; 128];
            assert_eq!(
                encode(&mut short[..capacity]),
                Err(StationFrameError::OutputTooSmall { required })
            );
            assert_eq!(short, [0xa5; 128]);
        }
        assert!(complete[required..].iter().all(|byte| *byte == 0xa5));
    }
}

#[test]
fn a_personal_selection_encodes_one_suite_and_its_capabilities() {
    let selected = SelectedRsn::personal(AssociationAkm::Psk, Spp::NotNegotiated, false);
    let bytes = selected.as_bytes();
    assert_eq!(bytes.len(), SELECTED_RSN_IE_LEN);
    let element = crate::security::rsn::RsnElement::parse(bytes).unwrap();
    assert!(
        element
            .akm_suites()
            .contains(crate::security::rsn::ieee_suite(2))
    );
    // SPP A-MSDU capable only.
    assert_eq!(&bytes[20..22], &[0, 4]);
    assert!(!selected.security().protects_management());

    let sae = SelectedRsn::personal(
        AssociationAkm::Sae(SaePwe::HashToElement),
        Spp::Negotiated,
        true,
    );
    // MFPC, MFPR and SPP A-MSDU capable, then the RSNXE announcing H2E.
    assert_eq!(&sae.as_bytes()[20..22], &[0xc0, 4]);
    assert_eq!(&sae.as_bytes()[22..], &[244, 1, 0x20]);
    let AssociationSecurity::Rsn(association) = sae.security() else {
        panic!("personal association");
    };
    assert_eq!(association.spp, Spp::Negotiated);
    assert!(sae.security().protects_management());
    assert!(SelectedRsn::OPEN.as_bytes().is_empty());
}

#[test]
fn a_resumed_sae_selection_lists_its_pmkid_before_the_rsnxe_once() {
    let selected = SelectedRsn::personal(
        AssociationAkm::Sae(SaePwe::HuntingAndPecking),
        Spp::Negotiated,
        true,
    );
    let resumed = selected.with_pmkid(Pmkid([0x5a; 16])).unwrap();
    let bytes = resumed.as_bytes();
    assert_eq!(bytes.len(), SELECTED_SECURITY_IES_CAPACITY);
    assert_eq!(&bytes[..2], &[48, 38]);
    assert_eq!(&bytes[2..22], &selected.as_bytes()[2..22]);
    assert_eq!(&bytes[22..24], &[1, 0]);
    assert_eq!(&bytes[24..40], &[0x5a; 16]);
    assert_eq!(&bytes[40..], &[244, 1, 0x20]);
    // A second PMKID and a non-SAE selection are refused.
    assert_eq!(resumed.with_pmkid(Pmkid([0xa5; 16])), None);
    assert_eq!(
        SelectedRsn::personal(AssociationAkm::Psk, Spp::Negotiated, false)
            .with_pmkid(Pmkid([0x5a; 16])),
        None
    );
}

#[test]
fn an_open_qos_data_frame_carries_its_tid_and_decodes_back() {
    let source =
        crate::address::OwnAddress::new(crate::address::IndividualAddress::new(LOCAL).unwrap());
    let bssid =
        crate::address::Bssid::new(crate::address::IndividualAddress::new(BSSID).unwrap()).unwrap();
    let frame = StaOpenDataFrame {
        source,
        bssid,
        destination: [0x30; 6],
        sequence_number: seq(0x12),
        priority: crate::qos::WmmUserPriority::UP5,
        peer_qos: true,
        ether_type: 0x0800,
        payload: &[1, 2, 3],
    };
    let mut output = [0_u8; 64];
    let len = frame.encode(&mut output).unwrap();
    assert_eq!(len, 26 + 8 + 3);
    assert_eq!(&output[..2], &[0x88, 0x01]);
    assert_eq!(&output[4..10], &BSSID);
    assert_eq!(&output[10..16], &LOCAL);
    assert_eq!(&output[16..22], &[0x30; 6]);
    assert_eq!(&output[22..24], &0x0120_u16.to_le_bytes());
    assert_eq!(&output[24..26], &[5, 0]);
    assert_eq!(
        &output[26..len],
        &[0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2, 3]
    );
    let header = crate::frame::MacHeader::decode(&output[..len]).unwrap();
    assert_eq!(header.qos().map(|qos| qos.tid().get()), Some(5));

    // Without QoS the same MSDU is a plain data frame; a short output is
    // refused.
    let plain = StaOpenDataFrame {
        peer_qos: false,
        ..frame
    };
    let len = plain.encode(&mut output).unwrap();
    assert_eq!(len, 24 + 8 + 3);
    assert_eq!(&output[..2], &[0x08, 0x01]);
    assert_eq!(
        plain.encode(&mut [0; 30]),
        Err(StationFrameError::OutputTooSmall { required: 35 })
    );
}
