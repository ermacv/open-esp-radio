use super::*;
use crate::sequence::seq;

#[test]
fn parses_every_common_info_field_across_the_word_boundary() {
    let bits = 4_u64
        | (0xabc << 4)
        | (1 << 16)
        | (1 << 17)
        | (2 << 18)
        | (3 << 20)
        | (1 << 22)
        | (5 << 23)
        | (1 << 26)
        | (1 << 27)
        | (42 << 28)
        | (2 << 34)
        | (1 << 36)
        | (0xbeef << 37)
        | (1 << 53)
        | (0x155 << 54)
        | (1 << 63);
    let common = parse_trigger_common_info(&bits.to_le_bytes()).unwrap();
    assert_eq!(common.trigger_type, TriggerType::BufferStatusReportPoll);
    assert_eq!(common.uplink_length, 0xabc);
    assert!(common.more_trigger_frames);
    assert!(common.carrier_sense_required);
    assert_eq!(common.uplink_bandwidth_encoding, 2);
    assert_eq!(common.gi_ltf, TriggerGiLtf::Reserved);
    assert!(common.mu_mimo_ltf_mode);
    assert_eq!(common.he_ltf_symbols_and_midamble_periodicity, 5);
    assert!(common.uplink_stbc);
    assert!(common.ldpc_extra_symbol_segment);
    assert_eq!(common.ap_tx_power_encoding, 42);
    assert_eq!(common.ap_tx_power, TriggerApTxPower::Dbm(22));
    assert_eq!(common.pre_fec_padding_factor_encoding, 2);
    assert_eq!(common.pre_fec_padding_factor, 2);
    assert!(common.packet_extension_disambiguity);
    assert_eq!(common.uplink_spatial_reuse, 0xbeef);
    assert!(common.doppler);
    assert_eq!(common.uplink_he_sig_a2_reserved, 0x155);
    assert!(common.trailing_reserved);
}

#[test]
fn admits_trigger_mac_header_and_borrows_user_tail() {
    let mut frame = [0_u8; 29];
    frame[0] = 0x24;
    frame[1] = 0x08;
    frame[2..4].copy_from_slice(&0x1234_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    frame[10..16].copy_from_slice(&[7, 8, 9, 10, 11, 12]);
    frame[16] = 1;
    frame[24..].copy_from_slice(&[13, 14, 15, 16, 17]);
    let trigger = parse_trigger_frame(&frame).unwrap();
    assert_eq!(trigger.frame_control, 0x0824);
    assert_eq!(trigger.duration, 0x1234);
    assert_eq!(trigger.receiver_address, [1, 2, 3, 4, 5, 6]);
    assert_eq!(trigger.transmitter_address, [7, 8, 9, 10, 11, 12]);
    assert_eq!(
        trigger.common.trigger_type,
        TriggerType::BeamformingReportPoll
    );
    assert_eq!(trigger.user_info_and_padding, [13, 14, 15, 16, 17]);
}

#[test]
fn basic_trigger_encoder_round_trips_every_owned_field() {
    let encoded = BasicTriggerFrameEncoding {
        duration: 0x1234,
        receiver_address: [1, 2, 3, 4, 5, 6],
        transmitter_address: [7, 8, 9, 10, 11, 12],
        common: TriggerCommonEncoding {
            uplink_length: 0xabc,
            more_trigger_frames: true,
            carrier_sense_required: true,
            uplink_bandwidth_encoding: 0,
            gi_ltf: TriggerGiLtf::TwoLtf1600Ns,
            mu_mimo_ltf_mode: true,
            he_ltf_symbols_and_midamble_periodicity: 5,
            uplink_stbc: true,
            ldpc_extra_symbol_segment: true,
            ap_tx_power_encoding: 42,
            pre_fec_padding_factor_encoding: 2,
            packet_extension_disambiguity: true,
            uplink_spatial_reuse: 0xbeef,
            doppler: true,
            uplink_he_sig_a2_reserved: 0x155,
            trailing_reserved: true,
        },
        user: TriggerScheduledUserEncoding {
            association_id: 0x234,
            ru_allocation_region: true,
            ru_allocation: 53,
            coding_type: true,
            mcs: 9,
            dcm: true,
            starting_spatial_stream_encoding: 2,
            spatial_stream_count_encoding: 3,
            target_rssi_encoding: 0x55,
            reserved: true,
        },
        dependent: TriggerBasicDependentInfo {
            mpdu_mu_spacing_factor: 3,
            tid_aggregation_limit: 5,
            reserved: true,
            preferred_access_category: 2,
        },
    };
    let mut bytes = [0xaa; TRIGGER_BASIC_FRAME_LEN];
    assert_eq!(encoded.encode(&mut bytes), Ok(TRIGGER_BASIC_FRAME_LEN));

    let trigger = parse_trigger_frame(&bytes).unwrap();
    assert_eq!(trigger.frame_control, 0x0024);
    assert_eq!(trigger.duration, encoded.duration);
    assert_eq!(trigger.receiver_address, encoded.receiver_address);
    assert_eq!(trigger.transmitter_address, encoded.transmitter_address);
    assert_eq!(trigger.common.trigger_type, TriggerType::Basic);
    assert_eq!(trigger.common.uplink_length, encoded.common.uplink_length);
    assert_eq!(
        trigger.common.uplink_bandwidth_encoding,
        encoded.common.uplink_bandwidth_encoding
    );
    assert_eq!(trigger.common.gi_ltf, encoded.common.gi_ltf);
    assert_eq!(
        trigger.common.he_ltf_symbols_and_midamble_periodicity,
        encoded.common.he_ltf_symbols_and_midamble_periodicity
    );
    assert_eq!(
        trigger.common.uplink_he_sig_a2_reserved,
        encoded.common.uplink_he_sig_a2_reserved
    );

    let mut users = trigger.users();
    let user = users.next().unwrap().unwrap();
    assert_eq!(users.next(), None);
    let parsed_user = parse_trigger_user_spatial_stream(user.user_info).unwrap();
    assert_eq!(parsed_user.aid12, encoded.user.association_id);
    assert_eq!(parsed_user.ru_allocation, encoded.user.ru_allocation);
    assert_eq!(parsed_user.coding_type, encoded.user.coding_type);
    assert_eq!(parsed_user.mcs, encoded.user.mcs);
    assert_eq!(parsed_user.dcm, encoded.user.dcm);
    assert_eq!(
        parsed_user.starting_spatial_stream_encoding,
        encoded.user.starting_spatial_stream_encoding
    );
    assert_eq!(
        parsed_user.spatial_stream_count_encoding,
        encoded.user.spatial_stream_count_encoding
    );
    assert_eq!(
        parsed_user.target_rssi_encoding,
        encoded.user.target_rssi_encoding
    );
    assert_eq!(
        parse_trigger_basic_dependent(user.dependent_info),
        Ok(encoded.dependent)
    );
}

#[test]
fn basic_trigger_encoder_rejects_truncation_and_unbounded_fields() {
    let valid = BasicTriggerFrameEncoding {
        duration: 0,
        receiver_address: [0xff; 6],
        transmitter_address: [0; 6],
        common: TriggerCommonEncoding {
            uplink_length: 1,
            more_trigger_frames: false,
            carrier_sense_required: false,
            uplink_bandwidth_encoding: 0,
            gi_ltf: TriggerGiLtf::OneLtf1600Ns,
            mu_mimo_ltf_mode: false,
            he_ltf_symbols_and_midamble_periodicity: 0,
            uplink_stbc: false,
            ldpc_extra_symbol_segment: false,
            ap_tx_power_encoding: 20,
            pre_fec_padding_factor_encoding: 1,
            packet_extension_disambiguity: false,
            uplink_spatial_reuse: 0,
            doppler: false,
            uplink_he_sig_a2_reserved: 0x1ff,
            trailing_reserved: false,
        },
        user: TriggerScheduledUserEncoding {
            association_id: 1,
            ru_allocation_region: false,
            ru_allocation: 61,
            coding_type: false,
            mcs: 0,
            dcm: false,
            starting_spatial_stream_encoding: 0,
            spatial_stream_count_encoding: 0,
            target_rssi_encoding: 0x7f,
            reserved: false,
        },
        dependent: TriggerBasicDependentInfo {
            mpdu_mu_spacing_factor: 0,
            tid_aggregation_limit: 0,
            reserved: false,
            preferred_access_category: 0,
        },
    };
    assert_eq!(
        valid.encode(&mut [0; TRIGGER_BASIC_FRAME_LEN - 1]),
        Err(TriggerEncodeError::OutputTooSmall {
            required: TRIGGER_BASIC_FRAME_LEN
        })
    );

    let mut invalid = valid;
    invalid.user.association_id = 0;
    assert_eq!(
        invalid.encode(&mut [0; TRIGGER_BASIC_FRAME_LEN]),
        Err(TriggerEncodeError::AssociationIdOutOfRange)
    );

    let mut invalid = valid;
    invalid.common.uplink_length = 0x1000;
    assert_eq!(
        invalid.encode(&mut [0; TRIGGER_BASIC_FRAME_LEN]),
        Err(TriggerEncodeError::UplinkLengthOutOfRange)
    );
}

#[test]
fn iterates_basic_users_and_stops_at_the_exact_blob_padding_marker() {
    let bytes = [
        0x34, 0x02, 0x00, 0x00, 0x01, 0xa5, // AID 0x234 + Basic suffix
        0x56, 0x04, 0x00, 0x00, 0x02, 0x5a, // AID 0x456 + Basic suffix
        0xff, 0xef, 0x0f, 0x00, 0x00, // AID 0xfff + RU allocation 0x7f
        0xaa, 0xbb,
    ];
    let mut users = TriggerUserIterator::new(TriggerType::Basic, &bytes);

    let first = users.next().unwrap().unwrap();
    assert_eq!(first.aid12(), 0x234);
    assert_eq!(first.user_info.len(), TRIGGER_USER_INFO_LEN);
    assert_eq!(first.dependent_info, [0xa5]);

    let second = users.next().unwrap().unwrap();
    assert_eq!(second.aid12(), 0x456);
    assert_eq!(second.dependent_info, [0x5a]);

    assert_eq!(users.next(), None);
    assert_eq!(users.padding(), &bytes[12..]);
}

#[test]
fn applies_the_instruction_proven_user_strides() {
    let mu_bar = [
        0x34, 0x02, 0x00, 0x00, 0x01, 0x05, 0xa0, 0x30, 0x12, 0x56, 0x04, 0x00, 0x00, 0x02, 0x01,
        0xb0, 0x40, 0x23,
    ];
    let mut users = TriggerUserIterator::new(TriggerType::MultiUserBlockAckRequest, &mu_bar);
    let first = users.next().unwrap().unwrap();
    assert_eq!(first.aid12(), 0x234);
    assert_eq!(first.dependent_info, [0x05, 0xa0, 0x30, 0x12]);
    let second = users.next().unwrap().unwrap();
    assert_eq!(second.aid12(), 0x456);
    assert_eq!(second.dependent_info, [0x01, 0xb0, 0x40, 0x23]);
    assert_eq!(users.next(), None);

    let bfrp = [0x34, 0x02, 0x00, 0x00, 0x01];
    let only = TriggerUserIterator::new(TriggerType::BeamformingReportPoll, &bfrp)
        .next()
        .unwrap()
        .unwrap();
    assert!(only.dependent_info.is_empty());
}

#[test]
fn classifies_only_the_ru_groups_with_valid_blob_indices() {
    for (raw, resource_unit, one_based_index) in [
        (0, HeResourceUnit::Ru26, 1),
        (8, HeResourceUnit::Ru26, 9),
        (37, HeResourceUnit::Ru52, 1),
        (40, HeResourceUnit::Ru52, 4),
        (53, HeResourceUnit::Ru106, 1),
        (54, HeResourceUnit::Ru106, 2),
        (61, HeResourceUnit::Ru242, 1),
        (62, HeResourceUnit::Ru242, 2),
    ] {
        assert_eq!(
            TriggerRuAllocation::from_encoding(raw),
            Some(TriggerRuAllocation::Narrow {
                resource_unit,
                one_based_index,
            })
        );
    }
    for raw in [9, 36, 41, 52, 55, 60, 63, 68] {
        assert_eq!(TriggerRuAllocation::from_encoding(raw), None);
    }
    assert_eq!(
        TriggerRuAllocation::from_encoding(69),
        Some(TriggerRuAllocation::WiderThan242)
    );
    assert_eq!(
        TriggerRuAllocation::from_encoding(127),
        Some(TriggerRuAllocation::WiderThan242)
    );
    assert_eq!(TriggerRuAllocation::from_encoding(128), None);

    let bytes = [0x34, 0xa2, 0x06, 0x00, 0x01];
    let field = TriggerUserField {
        user_info: &bytes,
        dependent_info: &[],
    };
    assert_eq!(field.ru_allocation(), 53);
    assert_eq!(
        field.classified_ru_allocation(),
        Some(TriggerRuAllocation::Narrow {
            resource_unit: HeResourceUnit::Ru106,
            one_based_index: 1,
        })
    );
}

#[test]
fn nfrp_is_one_terminal_user_and_retains_following_padding() {
    let bytes = [0x34, 0x02, 0x00, 0x00, 0x01, 0xaa, 0xbb];
    let mut users = TriggerUserIterator::new(TriggerType::NgpaFeedbackReportPoll, &bytes);
    assert_eq!(users.next().unwrap().unwrap().aid12(), 0x234);
    assert_eq!(users.next(), None);
    assert_eq!(users.padding(), [0xaa, 0xbb]);
}

#[test]
fn unsupported_and_truncated_user_layouts_fail_closed() {
    let user = [0_u8; TRIGGER_USER_INFO_LEN];
    let mut unsupported =
        TriggerUserIterator::new(TriggerType::GroupcastMultiUserBlockAckRequest, &user);
    assert_eq!(
        unsupported.next(),
        Some(Err(TriggerParseError::UnsupportedUserLayout {
            trigger_type: TriggerType::GroupcastMultiUserBlockAckRequest,
        }))
    );
    assert_eq!(unsupported.next(), None);

    let mut truncated = TriggerUserIterator::new(TriggerType::Basic, &[0; TRIGGER_USER_INFO_LEN]);
    assert_eq!(
        truncated.next(),
        Some(Err(TriggerParseError::Truncated { required: 6 }))
    );
    assert_eq!(truncated.next(), None);
}

#[test]
fn rejects_other_control_subtypes() {
    let mut frame = [0_u8; TRIGGER_FRAME_MIN_LEN];
    frame[0] = 0xd4;
    assert_eq!(
        parse_trigger_frame(&frame),
        Err(TriggerParseError::NotTriggerFrame)
    );
}

#[test]
fn zero_pre_fec_encoding_means_factor_four_in_the_blob() {
    let common = parse_trigger_common_info(&[0; 8]).unwrap();
    assert_eq!(common.pre_fec_padding_factor_encoding, 0);
    assert_eq!(common.pre_fec_padding_factor, 4);
    assert_eq!(common.ap_tx_power, TriggerApTxPower::Dbm(-20));
}

#[test]
fn parses_ru_and_spatial_stream_views_of_user_info() {
    let bytes = [0xa5, 0xbb, 0xd6, 0xeb, 0x52];
    let ru = parse_trigger_user_ru(&bytes).unwrap();
    assert_eq!(ru.aid12, 0xba5);
    assert!(ru.ru_allocation_region);
    assert_eq!(ru.ru_allocation, 53);
    assert!(ru.coding_type);
    assert_eq!(ru.mcs, 14);
    assert!(ru.dcm);
    assert_eq!(ru.number_of_ra_ru, 26);
    assert!(ru.more_ra_ru);
    assert_eq!(ru.target_rssi, TriggerTargetRssi::Dbm(-28));
    assert!(!ru.reserved);

    let ss = parse_trigger_user_spatial_stream(&bytes).unwrap();
    assert_eq!(ss.starting_spatial_stream_encoding, 2);
    assert_eq!(ss.starting_spatial_stream, 3);
    assert_eq!(ss.spatial_stream_count_encoding, 7);
    assert_eq!(ss.spatial_stream_count, 8);
}

#[test]
fn target_rssi_reserved_encoding_stays_distinct_from_dbm() {
    let user = parse_trigger_user_ru(&[0, 0, 0, 0, 0x7f]).unwrap();
    assert_eq!(user.target_rssi, TriggerTargetRssi::Reserved);
    let user = parse_trigger_user_ru(&[0, 0, 0, 0, 0]).unwrap();
    assert_eq!(user.target_rssi, TriggerTargetRssi::Dbm(-110));
}

#[test]
fn parses_all_dependent_user_forms() {
    let basic = parse_trigger_basic_dependent(&[0xd6]).unwrap();
    assert_eq!(basic.mpdu_mu_spacing_factor, 2);
    assert_eq!(basic.tid_aggregation_limit, 5);
    assert!(!basic.reserved);
    assert_eq!(basic.preferred_access_category, 3);

    assert_eq!(parse_trigger_bfrp_dependent(&[0xa5]).unwrap(), 0xa5);

    let mu_bar = parse_trigger_mu_bar_dependent(&[0x05, 0xa0, 0x30, 0x12]).unwrap();
    assert_eq!(mu_bar.bar_control, 0xa005);
    assert!(mu_bar.ack_policy);
    assert_eq!(mu_bar.bar_type, 2);
    assert_eq!(mu_bar.tid, 10);
    assert_eq!(mu_bar.bar_information, 0x1230);
    assert_eq!(mu_bar.starting_sequence_number, seq(0x123));

    let nfrp = parse_trigger_nfrp_user(&[0x34, 0xa2, 0xb5, 0x6b, 0xd5]).unwrap();
    assert_eq!(nfrp.starting_aid, 0x234);
    assert_eq!(nfrp.reserved_9, 0x15a);
    assert_eq!(nfrp.feedback_type, 13);
    assert_eq!(nfrp.reserved_7, 0x35);
    assert_eq!(nfrp.uplink_target_rssi, 0x55);
    assert!(nfrp.multiplexing);
}

#[test]
fn parses_trs_and_uph_control_without_hiding_unowned_bits() {
    let trs_bits = 0x2a_u32
        | (17 << 6)
        | (1 << 11)
        | (61 << 12)
        | (15 << 19)
        | (20 << 24)
        | (2 << 29)
        | (1 << 31);
    let trs = parse_trigger_response_scheduling_control(&trs_bits.to_le_bytes()).unwrap();
    assert_eq!(trs.control_id, 0x2a);
    assert_eq!(trs.uplink_data_symbols, 17);
    assert!(trs.ru_allocation_region);
    assert_eq!(trs.ru_allocation, 61);
    assert_eq!(trs.ap_tx_power, TriggerApTxPower::Dbm(10));
    assert_eq!(trs.uplink_target_rssi, TriggerTargetRssi::Dbm(-50));
    assert_eq!(trs.mcs, 2);
    assert!(trs.trailing_reserved);

    let uph_bits = 0x15_u32 | (23 << 6) | (1 << 11) | (2 << 12) | (0x2aa << 14);
    let uph = parse_uplink_power_headroom_control(&uph_bits.to_le_bytes()).unwrap();
    assert_eq!(uph.control_id, 0x15);
    assert_eq!(uph.uplink_power_headroom, 23);
    assert!(uph.minimum_transmit_power);
    assert_eq!(uph.reserved, 2);
    assert_eq!(uph.unparsed_upper_bits, 0x2aa);
}

#[test]
fn every_parser_rejects_a_truncated_field() {
    assert_eq!(
        parse_trigger_common_info(&[0; 7]),
        Err(TriggerParseError::Truncated { required: 8 })
    );
    assert_eq!(
        parse_trigger_user_ru(&[0; 4]),
        Err(TriggerParseError::Truncated { required: 5 })
    );
    assert_eq!(
        parse_trigger_response_scheduling_control(&[0; 3]),
        Err(TriggerParseError::Truncated { required: 4 })
    );
}
