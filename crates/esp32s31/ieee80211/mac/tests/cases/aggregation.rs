use crate::*;

#[test]
fn he_ampdu_density_and_empty_delimiters_match_complete_blob_integer_policy() {
    let expected_microseconds = [0, 1, 1, 1, 2, 4, 8, 16];
    for (encoding, expected) in expected_microseconds.into_iter().enumerate() {
        let density = HtAmpduDensity::from_ampdu_parameters((encoding as u8) << 2);
        assert_eq!(density.encoding(), encoding as u8);
        assert_eq!(density.vendor_integer_microseconds(), expected);
    }

    let ordinary = HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns);
    assert_eq!(
        ordinary.minimum_ampdu_subframe_bytes(HtAmpduDensity::SixteenMicroseconds),
        230
    );
    assert_eq!(
        ordinary.ampdu_empty_delimiters(28, HtAmpduDensity::SixteenMicroseconds),
        Some(50)
    );
    assert_eq!(
        ordinary.ampdu_empty_delimiters(28, HtAmpduDensity::NoRestriction),
        Some(0)
    );

    let dcm = HeRate::bcc_dcm(HeBccDcmMcs::Mcs3, HeGuardIntervalAndLtf::TwoLtf800Ns);
    assert_eq!(
        dcm.minimum_ampdu_subframe_bytes(HtAmpduDensity::SixteenMicroseconds),
        35
    );
    assert_eq!(
        dcm.ampdu_empty_delimiters(28, HtAmpduDensity::SixteenMicroseconds),
        Some(1)
    );
    assert_eq!(
        dcm.ampdu_empty_delimiters(0, HtAmpduDensity::SixteenMicroseconds),
        None
    );
}

#[test]
fn he_default_apep_limit_matches_rom_and_the_blob_dcm_branch() {
    assert_eq!(
        HeRate::new(HeMcs::Mcs0, HeGuardIntervalAndLtf::OneLtf800Ns).maximum_default_apep_bytes(),
        3_700
    );
    assert_eq!(
        HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns).maximum_default_apep_bytes(),
        50_000
    );
    assert_eq!(
        HeRate::new(HeMcs::Mcs6, HeGuardIntervalAndLtf::TwoLtf1600Ns).maximum_default_apep_bytes(),
        31_500
    );
    assert_eq!(
        HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::FourLtf3200Ns).maximum_default_apep_bytes(),
        42_000
    );

    // Complete ppCheckTxHEAMPDUlength halves the selected rate/GI limit when
    // descriptor-state bit 15 requests DCM.
    assert_eq!(
        HeRate::bcc_dcm(HeBccDcmMcs::Mcs0, HeGuardIntervalAndLtf::TwoLtf800Ns)
            .maximum_default_apep_bytes(),
        1_850
    );
    assert_eq!(
        HeRate::bcc_dcm(HeBccDcmMcs::Mcs3, HeGuardIntervalAndLtf::FourLtf3200Ns)
            .maximum_default_apep_bytes(),
        6_400
    );
}

#[test]
fn he_ampdu_config_rejects_an_apep_above_the_selected_rate_limit() {
    let gi_1600 = HeRate::ldpc(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf1600Ns);
    assert!(
        HeAmpduTxConfig::new(gi_1600, 27, 47_000, 31, HtAmpduDensity::NoRestriction,).is_some()
    );
    assert!(
        HeAmpduTxConfig::new(gi_1600, 27, 47_001, 32, HtAmpduDensity::NoRestriction,).is_none()
    );

    let gi_800 = HeRate::ldpc(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns);
    assert!(HeAmpduTxConfig::new(gi_800, 27, 50_000, 32, HtAmpduDensity::NoRestriction,).is_some());
    assert!(HeAmpduTxConfig::new(gi_800, 27, 50_001, 32, HtAmpduDensity::NoRestriction,).is_none());
}

#[test]
fn he_nonzero_edca_txop_apep_limits_match_the_complete_blob_producer() {
    // Complete rx11AXRate2AMPDULimit_update output for the standard WMM
    // voice TXOP of 47 * 32 us. Rows are 0.8/1.6/3.2-us GI.
    const VOICE_47: [[u32; 10]; 3] = [
        [
            1_469, 2_992, 4_490, 6_039, 9_061, 12_082, 13_593, 15_104, 18_125, 20_139,
        ],
        [
            1_386, 2_824, 4_238, 5_701, 8_552, 11_404, 12_830, 14_256, 17_108, 19_009,
        ],
        [
            1_240, 2_527, 3_792, 5_101, 7_653, 10_205, 11_481, 12_757, 15_309, 17_011,
        ],
    ];
    // Complete producer output for the standard WMM video TXOP of 94 * 32 us.
    const VIDEO_94: [[u32; 10]; 3] = [
        [
            3_086, 6_227, 9_342, 12_509, 18_765, 25_021, 28_149, 31_277, 37_533, 41_704,
        ],
        [
            2_914, 5_879, 8_821, 11_811, 17_717, 23_624, 26_578, 29_531, 35_438, 39_376,
        ],
        [
            2_615, 5_276, 7_916, 10_600, 15_901, 21_203, 23_854, 26_505, 31_806, 35_341,
        ],
    ];
    let profiles = [
        HeGuardIntervalAndLtf::TwoLtf800Ns,
        HeGuardIntervalAndLtf::TwoLtf1600Ns,
        HeGuardIntervalAndLtf::FourLtf3200Ns,
    ];

    for (row, guard_interval_and_ltf) in profiles.into_iter().enumerate() {
        for mcs_index in 0..10 {
            let rate = HeRate::new(
                HeMcs::from_index(mcs_index as u8).unwrap(),
                guard_interval_and_ltf,
            );
            assert_eq!(
                rate.maximum_apep_bytes(HeEdcaTxopLimit::from_units_32_us(47).unwrap()),
                VOICE_47[row][mcs_index]
            );
            assert_eq!(
                rate.maximum_apep_bytes(HeEdcaTxopLimit::from_units_32_us(94).unwrap()),
                VIDEO_94[row][mcs_index]
            );
        }
    }

    // Both 0.8-us encodings select the first producer row.
    assert_eq!(
        HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::OneLtf800Ns)
            .maximum_apep_bytes(HeEdcaTxopLimit::from_units_32_us(47).unwrap()),
        VOICE_47[0][9]
    );
    // Complete ppCheckTxHEAMPDUlength halves either generated table for DCM.
    assert_eq!(
        HeRate::bcc_dcm(HeBccDcmMcs::Mcs3, HeGuardIntervalAndLtf::TwoLtf1600Ns)
            .maximum_apep_bytes(HeEdcaTxopLimit::from_units_32_us(94).unwrap()),
        VIDEO_94[1][3] / 2
    );
}

#[test]
fn he_checked_apep_producer_matches_positive_blob_domain_and_rejects_wrap() {
    let profiles = [
        (HeGuardIntervalAndLtf::TwoLtf800Ns, 31.2_f32, 13.6_f32),
        (HeGuardIntervalAndLtf::TwoLtf1600Ns, 32.0_f32, 14.4_f32),
        (HeGuardIntervalAndLtf::FourLtf3200Ns, 40.0_f32, 16.0_f32),
    ];
    let data_bits_per_symbol = [117_i32, 234, 351, 468, 702, 936, 1_053, 1_170, 1_404, 1_560];
    let estimated_block_ack_us = [68_i32, 44, 44, 32, 32, 32, 32, 32, 32, 32];

    let mut rejected = 0_u16;
    let mut rejected_short_limits = [0_u16; 4];
    for units_32_us in 1_u16..=u16::from(u8::MAX) {
        let txop = HeEdcaTxopLimit::from_units_32_us(units_32_us).unwrap();
        for (guard_interval_and_ltf, preamble_us, symbol_us) in profiles {
            for mcs_index in 0..10 {
                let data_symbols = (((i32::from(units_32_us) * 32 - 36)
                    - estimated_block_ack_us[mcs_index])
                    as f32
                    - preamble_us)
                    / symbol_us;
                // This is the complete blob's fsub/fdiv/fmadd/fcvt/div
                // instruction sequence used as the independent test oracle.
                let signed_expected = (data_bits_per_symbol[mcs_index] as f32)
                    .mul_add(data_symbols, -22.0_f32) as i32
                    / 8;
                let rate = HeRate::new(
                    HeMcs::from_index(mcs_index as u8).unwrap(),
                    guard_interval_and_ltf,
                );
                if signed_expected <= 0 {
                    rejected = rejected.saturating_add(1);
                    if let Some(count) =
                        rejected_short_limits.get_mut(usize::from(units_32_us.saturating_sub(1)))
                    {
                        *count = count.saturating_add(1);
                    }
                    assert_eq!(rate.checked_maximum_apep_bytes(txop), None);
                    assert_eq!(rate.maximum_apep_bytes(txop), 0);
                    assert!(
                        HeAmpduTxConfig::new_with_txop(
                            rate,
                            1,
                            1,
                            1,
                            HtAmpduDensity::NoRestriction,
                            txop,
                        )
                        .is_none()
                    );
                } else {
                    let expected = signed_expected as u32;
                    assert_eq!(rate.checked_maximum_apep_bytes(txop), Some(expected));
                    assert_eq!(rate.maximum_apep_bytes(txop), expected);
                }
            }
        }
    }
    assert_ne!(rejected, 0, "the short-TXOP wrap domain remains covered");
    assert!(
        rejected_short_limits.into_iter().all(|count| count != 0),
        "every AP-controlled 1..=4-unit limit covers a non-positive rate/GI budget"
    );
}

#[test]
fn zero_edca_txop_selects_the_rom_apep_table_for_every_he_rate() {
    let profiles = [
        HeGuardIntervalAndLtf::OneLtf800Ns,
        HeGuardIntervalAndLtf::TwoLtf800Ns,
        HeGuardIntervalAndLtf::TwoLtf1600Ns,
        HeGuardIntervalAndLtf::FourLtf3200Ns,
    ];
    for guard_interval_and_ltf in profiles {
        for mcs_index in 0..10 {
            let rate = HeRate::new(
                HeMcs::from_index(mcs_index).unwrap(),
                guard_interval_and_ltf,
            );
            assert_eq!(
                rate.maximum_apep_bytes(HeEdcaTxopLimit::DEFAULT),
                u32::from(rate.maximum_default_apep_bytes())
            );
        }
    }
}

#[test]
fn ht_peer_ampdu_parameters_keep_length_density_and_queue_spacing_together() {
    let expected_maximum = [0x1fff, 0x3fff, 0x7fff, 0xffff];
    for exponent in 0_u8..=3 {
        let parameters = HtPeerAmpduParameters::from_capability_byte(exponent | (6 << 2));
        assert_eq!(
            parameters.maximum_aggregate_bytes(),
            expected_maximum[usize::from(exponent)]
        );
        assert_eq!(parameters.density(), HtAmpduDensity::EightMicroseconds);
        assert_eq!(
            parameters.protection_spacing(),
            HtProtectionSpacing::Density6
        );
    }
}

#[test]
fn aggregate_config_updates_the_same_retry_geometry_for_ht_and_he() {
    let ht_rate = HtRate::new(
        HtMcs::Mcs7,
        HtGuardInterval::Short400Ns,
        HtChannelWidth::Mhz40,
    );
    let control = oer_esp32s31_ieee80211_mac::tx::TxControlFrame {
        protection: oer_esp32s31_ieee80211_mac::tx::protection::TxProtection::RtsCts {
            rate: oer_esp32s31_ieee80211_mac::tx::LegacyRate::Ofdm24M,
        },
        power_primary: 1,
        power_alternate: 2,
    };
    let mut ht = AmpduTxConfig::Ht(HtAmpduTxConfig::new(ht_rate, 1_000, 2).unwrap());
    ht.update_retained_retry(512, 1, 31, control);
    assert_eq!(ht.control(), control);
    assert_eq!(ht.aggregate_length(), 512);
    assert_eq!(ht.rate(), TxPhyRate::Ht(ht_rate));
    assert_eq!(ht.hardware_key_selector(), 0);
    assert!(matches!(
        ht,
        AmpduTxConfig::Ht(HtAmpduTxConfig {
            aggregate_length: 512,
            subframes: 1,
            contention_window: 31,
            ..
        })
    ));

    let he_rate = HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns);
    let mut he = AmpduTxConfig::He(
        HeAmpduTxConfig::new(he_rate, 7, 1_000, 2, HtAmpduDensity::NoRestriction).unwrap(),
    );
    he.update_retained_retry(640, 1, 63, control);
    assert_eq!(he.control(), control);
    assert_eq!(he.rate(), TxPhyRate::He(he_rate));
    assert_eq!(he.hardware_key_selector(), 0);
    assert!(matches!(
        he,
        AmpduTxConfig::He(HeAmpduTxConfig {
            aggregate_length: 640,
            subframes: 1,
            contention_window: 63,
            ..
        })
    ));
}
