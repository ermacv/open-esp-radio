use super::*;

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

/// The HE Capabilities and HE Operation elements of the vendor AP the peer
/// state test recovers, plus a capability that carries no PPE thresholds.
const SEED_CAPABILITY: [u8; 28] = [
    0xff, 0x1a, 0x23, 0x05, 0x00, 0x18, 0x12, 0x00, 0x10, 0x22, 0x20, 0x02, 0xc0, 0x0f, 0x41, 0x95,
    0x08, 0x00, 0xcc, 0x00, 0xfa, 0xff, 0xfa, 0xff, 0x19, 0x1c, 0xc7, 0x71,
];
const SEED_OPERATION: [u8; 9] = [0xff, 0x07, 0x24, 0x04, 0x00, 0x01, 0x1b, 0xfc, 0xff];

fn minimal_capability() -> [u8; 24] {
    let mut element = [0_u8; 24];
    element[..3].copy_from_slice(&[255, 22, 35]);
    element[20..24].copy_from_slice(&[0xfd, 0xff, 0xfd, 0xff]);
    element
}

#[test]
fn a_capability_announcing_ppe_thresholds_without_them_is_too_short() {
    let mut capability = minimal_capability();
    capability[15] |= 0x80;
    assert_eq!(
        parse_he20_peer_state(&capability, &SEED_OPERATION),
        Err(HeElementError::TooShort)
    );
    let mut one_byte = [0_u8; 25];
    one_byte[..24].copy_from_slice(&capability);
    one_byte[1] = 23;
    assert_eq!(
        parse_he20_peer_state(&one_byte, &SEED_OPERATION),
        Err(HeElementError::TooShort)
    );
    assert!(parse_he20_peer_state(&SEED_CAPABILITY, &SEED_OPERATION).is_ok());
}

#[test]
fn element_checks_keep_their_order() {
    assert_eq!(parse_he20_operation(&[]), Err(HeElementError::WrongElement));
    assert_eq!(parse_he20_operation(&[255]), Err(HeElementError::TooShort));
    assert_eq!(
        parse_he20_operation(&[255, 7, 36]),
        Err(HeElementError::LengthMismatch)
    );
    assert_eq!(
        parse_he20_operation(&[255, 1, 36]),
        Err(HeElementError::TooShort)
    );
    let mut wrong = SEED_OPERATION;
    wrong[2] = 35;
    assert_eq!(
        parse_he20_operation(&wrong),
        Err(HeElementError::WrongExtension)
    );
}

fn parse_every_element(bytes: &[u8]) {
    if let Ok(capability) = parse_he20_capabilities(bytes) {
        let _ = (
            capability.supports_bidirectional_mcs9(),
            capability.supports_one_ltf_800ns_gi(),
            capability.dcm_receive_constellation(),
        );
    }
    if let Ok(operation) = parse_he20_operation(bytes) {
        let _ = operation.effective_bss_color();
    }
    // The input as either element of the pair, with the other one valid,
    // and split at its middle into a capability and an operation.
    let _ = parse_he20_peer_state(bytes, &SEED_OPERATION);
    let _ = parse_he20_peer_state(&SEED_CAPABILITY, bytes);
    let (capability, operation) = bytes.split_at(bytes.len() / 2);
    if let Ok(state) = parse_he20_peer_state(capability, operation) {
        let _ = state.extended_range_single_user_permitted();
    }
}

fn run_element_parsers(count: usize) {
    assert!(parse_he20_capabilities(&SEED_CAPABILITY).is_ok());
    let minimal = minimal_capability();
    assert!(parse_he20_capabilities(&minimal).is_ok());
    assert!(parse_he20_operation(&SEED_OPERATION).is_ok());
    // The pair back to back, which the middle split of a mutated input meets.
    let pair = [SEED_CAPABILITY.as_slice(), SEED_OPERATION.as_slice()].concat();
    let seeds = [
        SEED_CAPABILITY.as_slice(),
        SEED_OPERATION.as_slice(),
        minimal.as_slice(),
        pair.as_slice(),
    ];
    crate::fuzz::inputs(&seeds, count, 64, parse_every_element);
}

#[test]
fn he_element_parsers_never_panic_on_received_bytes() {
    run_element_parsers(crate::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn he_element_parsers_never_panic_on_received_bytes_full() {
    run_element_parsers(crate::fuzz::FULL);
}

#[test]
fn a_sig_b_user_field_is_read_lsb_first_and_refused_past_the_stream() {
    // User field bits 0..21 set to 0x15_5555 starting at bit 18.
    let mut stream = [0_u8; 6];
    let field = 0x15_5555_u32;
    for bit in 0..21_u16 {
        if field & (1 << bit) != 0 {
            let at = 18 + bit;
            stream[usize::from(at / 8)] |= 1 << (at % 8);
        }
    }
    assert_eq!(
        read_sig_b_user_field(&stream, HE20_MU_SIG_B_COMMON_BITS),
        Some(field)
    );
    assert_eq!(
        read_sig_b_user_field(&stream[..4], HE20_MU_SIG_B_COMMON_BITS),
        None
    );
}
