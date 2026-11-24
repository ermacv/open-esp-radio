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
    assert_eq!(common.trigger_type(), TriggerType::BufferStatusReportPoll);
    assert_eq!(common.fields.uplink_length(), 0xabc);
    assert!(common.fields.more_trigger_frames());
    assert!(common.fields.carrier_sense_required());
    assert_eq!(common.fields.uplink_bandwidth_encoding(), 2);
    assert_eq!(common.gi_ltf(), TriggerGiLtf::Reserved);
    assert!(common.fields.mu_mimo_ltf_mode());
    assert_eq!(common.fields.he_ltf_symbols_and_midamble_periodicity(), 5);
    assert!(common.fields.uplink_stbc());
    assert!(common.fields.ldpc_extra_symbol_segment());
    assert_eq!(common.fields.ap_tx_power_encoding(), 42);
    assert_eq!(common.ap_tx_power, TriggerApTxPower::Dbm(22));
    assert_eq!(common.fields.pre_fec_padding_factor_encoding(), 2);
    assert_eq!(common.pre_fec_padding_factor, 2);
    assert!(common.fields.packet_extension_disambiguity());
    assert_eq!(common.fields.uplink_spatial_reuse(), 0xbeef);
    assert!(common.fields.doppler());
    assert_eq!(common.fields.uplink_he_sig_a2_reserved(), 0x155);
    assert!(common.fields.trailing_reserved());
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
    assert_eq!(trigger.frame_control.to_bytes(), [0x24, 0x08]);
    assert_eq!(trigger.duration, 0x1234);
    assert_eq!(trigger.receiver_address, [1, 2, 3, 4, 5, 6]);
    assert_eq!(trigger.transmitter_address, [7, 8, 9, 10, 11, 12]);
    assert_eq!(
        trigger.common.trigger_type(),
        TriggerType::BeamformingReportPoll
    );
    assert_eq!(trigger.user_info_and_padding, [13, 14, 15, 16, 17]);
    // The general address view reads the same RA and TA, and requires both.
    let fields = crate::frame::AddressFields::parse(&frame).unwrap();
    assert_eq!(fields.receiver(), trigger.receiver_address);
    assert_eq!(fields.transmitter(), Some(trigger.transmitter_address));
    assert_eq!(
        crate::frame::AddressFields::parse(&frame[..10]),
        Err(crate::frame::AddressFieldsError::Truncated)
    );
}

#[test]
fn basic_trigger_encoder_round_trips_every_owned_field() {
    let encoded = BasicTriggerFrameEncoding {
        duration: 0x1234,
        receiver_address: [1, 2, 3, 4, 5, 6],
        transmitter_address: [7, 8, 9, 10, 11, 12],
        common: TriggerCommonFields::from_bytes([0; 8])
            .with_uplink_length(0xabc)
            .and_then(|fields| fields.with_uplink_bandwidth_encoding(0))
            .and_then(|fields| fields.with_gi_ltf(TriggerGiLtf::TwoLtf1600Ns.encoding()))
            .and_then(|fields| fields.with_he_ltf_symbols_and_midamble_periodicity(5))
            .and_then(|fields| fields.with_ap_tx_power_encoding(42))
            .and_then(|fields| fields.with_pre_fec_padding_factor_encoding(2))
            .and_then(|fields| fields.with_uplink_spatial_reuse(0xbeef))
            .and_then(|fields| fields.with_uplink_he_sig_a2_reserved(0x155))
            .unwrap()
            .with_more_trigger_frames(true)
            .with_carrier_sense_required(true)
            .with_mu_mimo_ltf_mode(true)
            .with_uplink_stbc(true)
            .with_ldpc_extra_symbol_segment(true)
            .with_packet_extension_disambiguity(true)
            .with_doppler(true)
            .with_trailing_reserved(true),
        user: TriggerScheduledUserFields::from_bytes([0; 5])
            .with_aid12(0x234)
            .and_then(|fields| fields.with_ru_allocation(53))
            .and_then(|fields| fields.with_mcs(9))
            .and_then(|fields| fields.with_starting_spatial_stream_encoding(2))
            .and_then(|fields| fields.with_spatial_stream_count_encoding(3))
            .and_then(|fields| fields.with_target_rssi_encoding(0x55))
            .unwrap()
            .with_ru_allocation_region(true)
            .with_coding_type(true)
            .with_dcm(true)
            .with_reserved(true),
        dependent: TriggerBasicDependentInfo {
            mpdu_mu_spacing_factor: 3,
            tid_aggregation_limit: 5,
            reserved: true,
            preferred_access_category: 2,
        },
    };
    let mut bytes = [0xaa; TRIGGER_BASIC_FRAME_LEN];
    assert_eq!(encoded.encode(&mut bytes), Ok(TRIGGER_BASIC_FRAME_LEN));
    // The frame's bytes as the encoder writes them.
    assert_eq!(
        bytes,
        [
            0x24, 0x00, 0x34, 0x12, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a,
            0x0b, 0x0c, 0xc0, 0xab, 0xd3, 0xae, 0xfa, 0xdd, 0x77, 0xd5, 0x34, 0xb2, 0x36, 0x6b,
            0xd5, 0xb7,
        ]
    );

    let trigger = parse_trigger_frame(&bytes).unwrap();
    assert_eq!(trigger.frame_control.to_bytes(), [0x24, 0x00]);
    assert_eq!(trigger.duration, encoded.duration);
    assert_eq!(trigger.receiver_address, encoded.receiver_address);
    assert_eq!(trigger.transmitter_address, encoded.transmitter_address);
    assert_eq!(trigger.common.trigger_type(), TriggerType::Basic);
    assert_eq!(
        trigger.common.fields.uplink_length(),
        encoded.common.uplink_length()
    );
    assert_eq!(
        trigger.common.fields.uplink_bandwidth_encoding(),
        encoded.common.uplink_bandwidth_encoding()
    );
    assert_eq!(trigger.common.fields.gi_ltf(), encoded.common.gi_ltf());
    assert_eq!(
        trigger
            .common
            .fields
            .he_ltf_symbols_and_midamble_periodicity(),
        encoded.common.he_ltf_symbols_and_midamble_periodicity()
    );
    assert_eq!(
        trigger.common.fields.uplink_he_sig_a2_reserved(),
        encoded.common.uplink_he_sig_a2_reserved()
    );

    let mut users = trigger.users();
    let user = users.next().unwrap().unwrap();
    assert_eq!(users.next(), None);
    let parsed_user = parse_trigger_user_spatial_stream(user.user_info).unwrap();
    assert_eq!(parsed_user.fields.aid12(), encoded.user.aid12());
    assert_eq!(
        parsed_user.fields.ru_allocation(),
        encoded.user.ru_allocation()
    );
    assert_eq!(parsed_user.fields.coding_type(), encoded.user.coding_type());
    assert_eq!(parsed_user.fields.mcs(), encoded.user.mcs());
    assert_eq!(parsed_user.fields.dcm(), encoded.user.dcm());
    assert_eq!(
        parsed_user.fields.starting_spatial_stream_encoding(),
        encoded.user.starting_spatial_stream_encoding()
    );
    assert_eq!(
        parsed_user.fields.spatial_stream_count_encoding(),
        encoded.user.spatial_stream_count_encoding()
    );
    assert_eq!(
        parsed_user.fields.target_rssi_encoding(),
        encoded.user.target_rssi_encoding()
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
        common: TriggerCommonFields::from_bytes([0; 8])
            .with_uplink_length(1)
            .and_then(|fields| fields.with_gi_ltf(TriggerGiLtf::OneLtf1600Ns.encoding()))
            .and_then(|fields| fields.with_ap_tx_power_encoding(20))
            .and_then(|fields| fields.with_pre_fec_padding_factor_encoding(1))
            .and_then(|fields| fields.with_uplink_he_sig_a2_reserved(0x1ff))
            .unwrap(),
        user: TriggerScheduledUserFields::from_bytes([0; 5])
            .with_aid12(1)
            .and_then(|fields| fields.with_ru_allocation(61))
            .and_then(|fields| fields.with_target_rssi_encoding(0x7f))
            .unwrap(),
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
    invalid.user = invalid.user.with_aid12(0).unwrap();
    assert_eq!(
        invalid.encode(&mut [0; TRIGGER_BASIC_FRAME_LEN]),
        Err(TriggerEncodeError::AssociationIdOutOfRange)
    );

    // A length past the field's twelve bits is refused by its setter.
    assert_eq!(
        valid.common.with_uplink_length(0x1000),
        Err(oer_portable_wire::Error::ValueOutOfRange)
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
    assert_eq!(common.fields.pre_fec_padding_factor_encoding(), 0);
    assert_eq!(common.pre_fec_padding_factor, 4);
    assert_eq!(common.ap_tx_power, TriggerApTxPower::Dbm(-20));
}

#[test]
fn parses_ru_and_spatial_stream_views_of_user_info() {
    let bytes = [0xa5, 0xbb, 0xd6, 0xeb, 0x52];
    let ru = parse_trigger_user_ru(&bytes).unwrap();
    assert_eq!(ru.fields.aid12(), 0xba5);
    assert!(ru.fields.ru_allocation_region());
    assert_eq!(ru.fields.ru_allocation(), 53);
    assert!(ru.fields.coding_type());
    assert_eq!(ru.fields.mcs(), 14);
    assert!(ru.fields.dcm());
    assert_eq!(ru.fields.number_of_ra_ru(), 26);
    assert!(ru.fields.more_ra_ru());
    assert_eq!(ru.target_rssi, TriggerTargetRssi::Dbm(-28));
    assert!(!ru.fields.reserved());

    let ss = parse_trigger_user_spatial_stream(&bytes).unwrap();
    assert_eq!(ss.fields.starting_spatial_stream_encoding(), 2);
    assert_eq!(ss.starting_spatial_stream, 3);
    assert_eq!(ss.fields.spatial_stream_count_encoding(), 7);
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
    assert_eq!(mu_bar.bar_control.to_bytes(), [0x05, 0xa0]);
    assert!(mu_bar.ack_policy());
    assert_eq!(mu_bar.bar_type(), 2);
    assert_eq!(mu_bar.tid(), 10);
    assert_eq!(mu_bar.bar_information.to_bytes(), [0x30, 0x12]);
    assert_eq!(mu_bar.starting_sequence_number(), seq(0x123));

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
    assert_eq!(trs.fields.control_id(), 0x2a);
    assert_eq!(trs.fields.uplink_data_symbols(), 17);
    assert!(trs.fields.ru_allocation_region());
    assert_eq!(trs.fields.ru_allocation(), 61);
    assert_eq!(trs.ap_tx_power, TriggerApTxPower::Dbm(10));
    assert_eq!(trs.uplink_target_rssi, TriggerTargetRssi::Dbm(-50));
    assert_eq!(trs.fields.mcs(), 2);
    assert!(trs.fields.trailing_reserved());

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

#[test]
fn every_prefix_of_a_trigger_frame_is_handled() {
    let mut frame = [0xffu8; 40];
    frame[0] = 0x24;
    frame[1] = 0x00;
    for len in 0..=frame.len() {
        let bytes = &frame[..len];
        if let Ok(parsed) = parse_trigger_frame(bytes) {
            parsed.users().for_each(drop);
        }
        let _ = parse_trigger_common_info(bytes);
        let _ = parse_trigger_user_ru(bytes);
        let _ = parse_trigger_user_spatial_stream(bytes);
        let _ = parse_trigger_basic_dependent(bytes);
        let _ = parse_trigger_bfrp_dependent(bytes);
        let _ = parse_trigger_mu_bar_dependent(bytes);
        let _ = parse_trigger_nfrp_user(bytes);
        let _ = parse_trigger_response_scheduling_control(bytes);
        let _ = parse_uplink_power_headroom_control(bytes);
    }
}

#[test]
fn every_bounded_power_encoding_has_its_exact_dbm_value() {
    for code in 0..128 {
        let actual =
            TriggerTargetRssi::from_user_encoding(oer_portable_wire::U8::new(code).unwrap());
        let expected = if code == 127 {
            TriggerTargetRssi::Reserved
        } else {
            TriggerTargetRssi::Dbm((i16::from(code) - 110) as i8)
        };
        assert_eq!(actual, expected);
    }
    for code in 0..64 {
        assert_eq!(
            TriggerApTxPower::from_common_encoding(oer_portable_wire::U8::new(code).unwrap()),
            TriggerApTxPower::Dbm((i16::from(code) - 20) as i8)
        );
    }
    for code in 0..32 {
        let code_value = oer_portable_wire::U8::new(code).unwrap();
        assert_eq!(
            TriggerTargetRssi::from_trs_encoding(code_value),
            if code == 31 {
                TriggerTargetRssi::Reserved
            } else {
                TriggerTargetRssi::Dbm(((i16::from(code) - 45) * 2) as i8)
            }
        );
        assert_eq!(
            TriggerApTxPower::from_trs_encoding(code_value),
            if code == 31 {
                TriggerApTxPower::Reserved
            } else {
                TriggerApTxPower::Dbm(((i16::from(code) - 10) * 2) as i8)
            }
        );
    }
}

/// A MU-BAR's Ack Policy is its BAR Control's bit 0.
#[test]
fn a_mu_bar_ack_policy_is_its_own_bit() {
    let set = parse_trigger_mu_bar_dependent(&[0x05, 0xa0, 0x30, 0x12]).unwrap();
    let clear = parse_trigger_mu_bar_dependent(&[0x04, 0xa0, 0x30, 0x12]).unwrap();
    assert!(set.ack_policy());
    assert!(!clear.ack_policy());
}
