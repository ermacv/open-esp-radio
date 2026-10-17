use super::*;

fn write_bits(bytes: &mut [u8], bit_offset: u16, bit_width: u16, value: u32) {
    for input_bit in 0..bit_width {
        let destination_bit = bit_offset + input_bit;
        let destination = &mut bytes[usize::from(destination_bit / 8)];
        let mask = 1 << (destination_bit % 8);
        if value & (1 << input_bit) != 0 {
            *destination |= mask;
        } else {
            *destination &= !mask;
        }
    }
}

#[test]
fn parses_single_stream_mcs9_capability_without_optional_tails() {
    let mut element = [0_u8; 24];
    element[..3].copy_from_slice(&[255, 22, 35]);
    element[20..22].copy_from_slice(&0xfffd_u16.to_le_bytes());
    element[22..24].copy_from_slice(&0xfffd_u16.to_le_bytes());
    let capability = parse_he20_capabilities(&element).unwrap();
    assert_eq!(capability.receive_nss1, HeMcsNssSupport::Mcs0To9);
    assert_eq!(capability.transmit_nss1, HeMcsNssSupport::Mcs0To9);
    assert!(capability.supports_bidirectional_mcs9());
    assert!(!capability.supports_one_ltf_800ns_gi());
    assert_eq!(
        capability.dcm_receive_constellation(),
        HeDcmConstellation::NotSupported
    );
}

#[test]
fn decodes_complete_non_mimo_sig_b_user_and_terminal_sentinel() {
    let word = 0x10_0000 | 0x08_0000 | (9 << 15) | (1 << 14) | (5 << 11) | 0x234;
    assert_eq!(
        HeMuSigBNonMimoUser::decode(word),
        HeMuSigBNonMimoUser::Scheduled {
            station_id: 0x234,
            nsts: 5,
            beamformed: true,
            mcs: 9,
            dcm: true,
            ldpc: true,
        }
    );
    assert_eq!(
        HeMuSigBNonMimoUser::decode(0x001f_f7fe),
        HeMuSigBNonMimoUser::NonMuMimo
    );
}

#[test]
fn decodes_complete_mimo_sig_b_user_without_exposing_reserved_bit() {
    let word = 0x10_0000 | 0x08_0000 | (7 << 15) | (12 << 11) | 0x345;
    assert_eq!(
        HeMuSigBMimoUser::decode(word),
        HeMuSigBMimoUser {
            station_id: 0x345,
            spatial_configuration: 12,
            mcs: 7,
            ldpc: true,
        }
    );
}

#[test]
fn decodes_exact_rom_backed_he_sig_b_ru_allocations() {
    for (encoding, expected_users) in [9, 8, 8, 7, 8, 7, 7, 6, 8, 7, 7, 6, 7, 6, 6, 5]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            He20MuSigBRuAllocation::try_new(encoding as u8)
                .unwrap()
                .user_count(),
            expected_users
        );
    }

    let all_ru26 = He20MuSigBRuAllocation::try_new(0).unwrap();
    assert_eq!(all_ru26.user_count(), 9);
    for position in 0..all_ru26.user_count() {
        assert_eq!(
            all_ru26.user(position),
            Some(He20MuSigBRuUser {
                zero_based_position: position,
                resource_unit: HeResourceUnit::Ru26,
                multiplexed: 0,
            })
        );
    }

    let mixed = He20MuSigBRuAllocation::try_new(15).unwrap();
    let expected = [
        HeResourceUnit::Ru52,
        HeResourceUnit::Ru52,
        HeResourceUnit::Ru26,
        HeResourceUnit::Ru52,
        HeResourceUnit::Ru52,
    ];
    for (position, resource_unit) in expected.into_iter().enumerate() {
        assert_eq!(
            mixed.user(position as u8).unwrap().resource_unit,
            resource_unit
        );
    }
    assert_eq!(mixed.user(mixed.user_count()), None);
}

#[test]
fn decodes_every_computed_he20_sig_b_ru_class_boundary() {
    let cases = [
        (23, 0, 10, HeResourceUnit::Ru52, 7),
        (23, 2, 10, HeResourceUnit::Ru106, 7),
        (24, 0, 3, HeResourceUnit::Ru106, 0),
        (24, 1, 3, HeResourceUnit::Ru52, 0),
        (47, 5, 13, HeResourceUnit::Ru106, 7),
        (48, 2, 5, HeResourceUnit::Ru52, 0),
        (48, 4, 5, HeResourceUnit::Ru106, 0),
        (56, 0, 5, HeResourceUnit::Ru52, 0),
        (79, 0, 13, HeResourceUnit::Ru106, 7),
        (80, 4, 5, HeResourceUnit::Ru106, 0),
        (88, 2, 5, HeResourceUnit::Ru106, 0),
        (96, 1, 4, HeResourceUnit::Ru52, 0),
        (96, 3, 4, HeResourceUnit::Ru106, 0),
        (104, 0, 4, HeResourceUnit::Ru106, 0),
        (104, 1, 4, HeResourceUnit::Ru26, 0),
        (104, 2, 4, HeResourceUnit::Ru52, 0),
        (112, 3, 4, HeResourceUnit::Ru52, 0),
        (128, 0, 2, HeResourceUnit::Ru106, 0),
        (129, 0, 3, HeResourceUnit::Ru106, 0),
        (129, 1, 3, HeResourceUnit::Ru106, 1),
        (191, 7, 8, HeResourceUnit::Ru106, 1),
        (199, 7, 8, HeResourceUnit::Ru242, 7),
    ];
    for (encoding, position, user_count, resource_unit, multiplexed) in cases {
        let allocation = He20MuSigBRuAllocation::try_new(encoding).unwrap();
        assert_eq!(allocation.encoding(), encoding);
        assert_eq!(allocation.user_count(), user_count);
        assert_eq!(
            allocation.user(position),
            Some(He20MuSigBRuUser {
                zero_based_position: position,
                resource_unit,
                multiplexed,
            })
        );
    }
}

#[test]
fn rejects_reserved_and_non_he20_sig_b_ru_types() {
    for encoding in [113, 127, 217, 255] {
        assert_eq!(
            He20MuSigBRuAllocation::try_new(encoding),
            Err(He20MuSigBRuAllocationError::ReservedEncoding)
        );
    }
    for encoding in [200, 207, 208, 216] {
        assert_eq!(
            He20MuSigBRuAllocation::try_new(encoding),
            Err(He20MuSigBRuAllocationError::UnsupportedRuType)
        );
    }
}

#[test]
fn iterates_three_he20_non_mimo_users_across_pair_crc_tail_gap() {
    let words = [
        (1 << 20) | (3 << 15) | 0x123,
        (1 << 19) | (5 << 15) | 0x456,
        (1 << 14) | (7 << 15) | 0x321,
    ];
    let mut complete = [0_u8; 13];
    write_bits(&mut complete, 18, 21, words[0]);
    write_bits(&mut complete, 39, 21, words[1]);
    write_bits(&mut complete, 70, 21, words[2]);

    // 18 common + 52 for the first pair + 21 for the final user + ten
    // final CRC/tail bits. The blob user-count expression sees three.
    let mut users = He20MuSigBNonMimoUsers::try_new(&complete, 101).unwrap();
    assert_eq!(users.user_count(), 3);
    assert_eq!(users.len(), 3);
    for (expected_index, expected_offset, expected_word) in
        [(0, 18, words[0]), (1, 39, words[1]), (2, 70, words[2])]
    {
        let entry = users.next().unwrap();
        assert_eq!(entry.index, expected_index);
        assert_eq!(entry.bit_offset, expected_offset);
        assert_eq!(entry.raw, expected_word);
        assert_eq!(entry.user, HeMuSigBNonMimoUser::decode(expected_word));
    }
    assert_eq!(users.next(), None);
    assert_eq!(users.len(), 0);
}

#[test]
fn rejects_truncated_or_out_of_domain_he20_complete_streams() {
    assert_eq!(
        He20MuSigBNonMimoUsers::try_new(&[], 17),
        Err(He20MuSigBNonMimoStreamError::BitLengthBeforeFirstUser)
    );
    assert_eq!(
        He20MuSigBNonMimoUsers::try_new(&[0; 11], 101),
        Err(He20MuSigBNonMimoStreamError::CompleteBytesTooShort)
    );
    assert_eq!(
        He20MuSigBNonMimoUsers::try_new(&[0; 12], 90),
        Err(He20MuSigBNonMimoStreamError::IncompleteUserField)
    );
    assert_eq!(
        He20MuSigBNonMimoUsers::try_new(&[0; 35], 278),
        Err(He20MuSigBNonMimoStreamError::TooManyUsers)
    );
}

#[test]
fn binds_he20_common_ru_allocation_to_the_complete_user_count() {
    let mut four_users = [0_u8; 16];
    four_users[0] = 112;
    let users = He20MuSigBNonMimoUsers::try_new(&four_users, 122).unwrap();
    let allocation = users.ru_allocation().unwrap();
    assert_eq!(allocation.encoding(), 112);
    assert_eq!(allocation.user_count(), 4);

    four_users[0] = 0;
    assert_eq!(
        He20MuSigBNonMimoUsers::try_new(&four_users, 122)
            .unwrap()
            .ru_allocation(),
        Err(He20MuSigBRuStreamError::UserCountMismatch {
            stream_users: 4,
            allocation_users: 9,
        })
    );
    four_users[0] = 200;
    assert_eq!(
        He20MuSigBNonMimoUsers::try_new(&four_users, 122)
            .unwrap()
            .ru_allocation(),
        Err(He20MuSigBRuStreamError::Allocation(
            He20MuSigBRuAllocationError::UnsupportedRuType
        ))
    );
}

#[test]
fn iterates_the_four_non_linear_compressed_mimo_user_offsets() {
    let words = [
        (1 << 20) | (1 << 15) | (4 << 11) | 0x111,
        (2 << 15) | (4 << 11) | 0x222,
        (1 << 20) | (3 << 15) | (4 << 11) | 0x333,
        (4 << 15) | (4 << 11) | 0x444,
    ];
    let mut complete = [0_u8; 17];
    for (offset, word) in [0, 21, 52, 105].into_iter().zip(words) {
        write_bits(&mut complete, offset, 21, word);
    }

    let mut users = He20MuSigBMimoUsers::try_new(&complete, 136, 4).unwrap();
    let spatial = users.spatial_configuration().unwrap();
    assert_eq!(spatial.user_count(), 4);
    assert_eq!(spatial.encoding(), 4);
    assert_eq!(
        [0, 1, 2, 3].map(|index| spatial.nsts_for_user(index).unwrap()),
        [2, 2, 1, 1]
    );
    assert_eq!(spatial.total_nsts(), 6);
    assert_eq!(users.len(), 4);
    for (expected_offset, expected_word) in [0, 21, 52, 105].into_iter().zip(words) {
        let entry = users.next().unwrap();
        assert_eq!(entry.bit_offset, expected_offset);
        assert_eq!(entry.raw, expected_word);
        assert_eq!(entry.user, HeMuSigBMimoUser::decode(expected_word));
    }
    assert_eq!(users.next(), None);

    write_bits(&mut complete, 21, 21, words[1] ^ (1 << 11));
    assert_eq!(
        He20MuSigBMimoUsers::try_new(&complete, 136, 4)
            .unwrap()
            .spatial_configuration(),
        Err(He20MuSigBMimoSpatialError::InconsistentEncoding)
    );
}

#[test]
fn rejects_invalid_compressed_mimo_counts_lengths_and_storage() {
    assert_eq!(
        He20MuSigBMimoUsers::try_new(&[0; 17], 136, 0),
        Err(He20MuSigBMimoStreamError::UserCountOutOfRange)
    );
    assert_eq!(
        He20MuSigBMimoUsers::try_new(&[0; 17], 136, 5),
        Err(He20MuSigBMimoStreamError::UserCountOutOfRange)
    );
    assert_eq!(
        He20MuSigBMimoUsers::try_new(&[0; 16], 136, 4),
        Err(He20MuSigBMimoStreamError::CompleteBytesTooShort)
    );
    assert_eq!(
        He20MuSigBMimoUsers::try_new(&[0; 17], 135, 4),
        Err(He20MuSigBMimoStreamError::IncompleteUserField)
    );
    assert_eq!(
        He20MuSigBMimoUsers::try_new(&[0; 9], 72, 3),
        Err(He20MuSigBMimoStreamError::IncompleteUserField)
    );
    assert_eq!(HeMuMimoSpatialConfiguration::try_new(1, 0), None);
    assert_eq!(HeMuMimoSpatialConfiguration::try_new(4, 11), None);
    assert_eq!(
        HeMuMimoSpatialConfiguration::try_new(8, 0)
            .unwrap()
            .total_nsts(),
        8
    );
}

#[test]
fn parses_optional_one_ltf_800ns_gi_capability() {
    let mut element = [0_u8; 24];
    element[..3].copy_from_slice(&[255, 22, 35]);
    element[10] = 0x40;
    let capability = parse_he20_capabilities(&element).unwrap();
    assert!(capability.supports_one_ltf_800ns_gi());
    assert!(!capability.supports_ldpc_coding_in_payload());
}

#[test]
fn parses_payload_ldpc_independently_from_gi_and_dcm() {
    let mut element = [0_u8; 24];
    element[..3].copy_from_slice(&[255, 22, 35]);
    element[10] = 0x20;
    let capability = parse_he20_capabilities(&element).unwrap();
    assert!(!capability.supports_one_ltf_800ns_gi());
    assert!(capability.supports_ldpc_coding_in_payload());
    assert_eq!(
        capability.dcm_receive_constellation(),
        HeDcmConstellation::NotSupported
    );
}

#[test]
fn parses_independent_dcm_transmit_and_receive_constellations() {
    let mut element = [0_u8; 24];
    element[..3].copy_from_slice(&[255, 22, 35]);
    // Peer TX: QPSK (bits 1:0 = 2); peer RX: 16-QAM
    // (bits 4:3 = 3). NSS stays one when bits 2 and 5 are clear.
    element[12] = 0x1a;
    let capability = parse_he20_capabilities(&element).unwrap();
    assert_eq!(capability.dcm_transmit, HeDcmConstellation::Qpsk);
    assert_eq!(capability.dcm_receive, HeDcmConstellation::Qam16);
    assert!(capability.dcm_receive.supports_bpsk());
    assert!(capability.dcm_receive.supports_qpsk());
    assert!(capability.dcm_receive.supports_16qam());
}

#[test]
fn parses_stbc_and_independent_cqi_feedback_capabilities() {
    let mut element = [0_u8; 24];
    element[..3].copy_from_slice(&[255, 22, 35]);
    element[11] = 0x0c;
    element[15] = 0x1c;
    element[18] = 0x02;
    let capability = parse_he20_capabilities(&element).unwrap();
    assert!(capability.stbc_transmit_under_80_mhz);
    assert!(capability.stbc_receive_under_80_mhz);
    assert!(capability.triggered_su_beamforming_feedback);
    assert!(capability.triggered_mu_beamforming_partial_bandwidth_feedback);
    assert!(capability.triggered_cqi_feedback);
    assert!(capability.non_triggered_cqi_feedback);
}

#[test]
fn decodes_the_vendor_s31_sta_stbc_and_cqi_advertisement() {
    let capability = [
        0xff, 0x16, 0x23, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0,
        0x1f, 0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
    ];
    let capability = parse_he20_capabilities(&capability).unwrap();
    assert!(!capability.stbc_transmit_under_80_mhz);
    assert!(capability.stbc_receive_under_80_mhz);
    assert_eq!(capability.dcm_transmit, HeDcmConstellation::Qam16);
    assert_eq!(capability.dcm_receive, HeDcmConstellation::Qam16);
    assert!(capability.triggered_su_beamforming_feedback);
    assert!(capability.triggered_mu_beamforming_partial_bandwidth_feedback);
    assert!(capability.triggered_cqi_feedback);
    assert!(capability.non_triggered_cqi_feedback);
}

#[test]
fn parses_disabled_partial_bss_color() {
    let element = [255, 7, 36, 0, 0, 0, 0xc5, 0xfd, 0xff];
    let operation = parse_he20_operation(&element).unwrap();
    assert_eq!(operation.bss_color, 5);
    assert!(!operation.bss_color_enabled);
    assert!(operation.partial_bss_color);
    assert_eq!(operation.effective_bss_color(), 0);
    assert_eq!(operation.basic_mcs_nss_map, 0xfffd);
}

#[test]
fn disabled_color_matches_vendor_effective_tx_color() {
    let element = [255, 7, 36, 0, 0, 0, 0xae, 0xfd, 0xff];
    let operation = parse_he20_operation(&element).unwrap();
    assert_eq!(operation.bss_color, 46);
    assert!(!operation.bss_color_enabled);
    assert!(!operation.partial_bss_color);
    assert_eq!(operation.effective_bss_color(), 0);
}

#[test]
fn recovers_vendor_ap_he20_peer_state() {
    let capability = [
        0xff, 0x1a, 0x23, 0x05, 0x00, 0x18, 0x12, 0x00, 0x10, 0x22, 0x20, 0x02, 0xc0, 0x0f, 0x41,
        0x95, 0x08, 0x00, 0xcc, 0x00, 0xfa, 0xff, 0xfa, 0xff, 0x19, 0x1c, 0xc7, 0x71,
    ];
    let operation = [0xff, 0x07, 0x24, 0x04, 0x00, 0x01, 0x1b, 0xfc, 0xff];
    let state = parse_he20_peer_state(&capability, &operation).unwrap();
    assert_eq!(state.packet_padding_eight_us, 2);
    assert_eq!(state.default_packet_extension_duration, 4);
    assert_eq!(state.bss_color, 27);
    assert!(state.bss_color_enabled);
    assert!(!state.partial_bss_color);
    assert_eq!(state.basic_mcs_nss_map, 0xfffc);
    assert_eq!(state.rts_threshold, None);
    assert!(state.extended_range_single_user_disabled);
    assert!(!state.extended_range_single_user_permitted());
    assert!(
        !parse_he20_capabilities(&capability)
            .unwrap()
            .supports_one_ltf_800ns_gi()
    );
    assert_eq!(
        parse_he20_capabilities(&capability)
            .unwrap()
            .dcm_receive_constellation(),
        HeDcmConstellation::NotSupported
    );
}
