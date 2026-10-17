use crate::*;

#[test]
fn legacy_rate_codes_preserve_the_non_monotonic_hardware_encoding() {
    assert_eq!(LegacyRate::Dsss1MLong.code(), 0x00);
    assert_eq!(LegacyRate::Ofdm48M.code(), 0x08);
    assert_eq!(LegacyRate::Ofdm6M.code(), 0x0b);
    assert_eq!(LegacyRate::Ofdm54M.code(), 0x0c);
    assert_eq!(LegacyRate::Ofdm9M.code(), 0x0f);
    assert_eq!(LegacyRate::Ofdm54M.nominal_kbps(), 54_000);
}

#[test]
fn ht_rate_codes_keep_gi_separate_from_power_lookup_and_width() {
    let lgi = HtRate::new(
        HtMcs::Mcs7,
        HtGuardInterval::Long800Ns,
        HtChannelWidth::Mhz40,
    );
    let sgi = HtRate::new(
        HtMcs::Mcs7,
        HtGuardInterval::Short400Ns,
        HtChannelWidth::Mhz40,
    );
    assert_eq!(lgi.code(), 23);
    assert_eq!(sgi.code(), 33);
    assert_eq!(lgi.power_lookup_code(), 23);
    assert_eq!(sgi.power_lookup_code(), 23);
    assert_eq!(lgi.nominal_kbps(), 135_000);
    assert_eq!(sgi.nominal_kbps(), 150_000);
    assert_eq!(lgi.vendor_ampdu_byte_limit(), Some(65_535));
    assert_eq!(sgi.vendor_ampdu_byte_limit(), None);
    assert_eq!(sgi.vendor_control_rate(), LegacyRate::Ofdm24M);
    assert_eq!(sgi.vendor_retry_rate(0), Some(TxPhyRate::Ht(sgi)));
    assert_eq!(
        sgi.vendor_retry_rate(2),
        Some(TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs6,
            HtGuardInterval::Long800Ns,
            HtChannelWidth::Mhz40,
        ))),
    );
    assert_eq!(
        sgi.vendor_retry_rate(4),
        Some(TxPhyRate::Legacy(LegacyRate::Ofdm6M)),
    );

    assert_eq!(
        HtRate::new(
            HtMcs::Mcs0,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz20,
        )
        .vendor_control_rate(),
        LegacyRate::Ofdm6M,
    );
    assert_eq!(
        HtRate::new(
            HtMcs::Mcs0,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz20,
        )
        .vendor_ampdu_byte_limit(),
        Some(9_600),
    );
    assert_eq!(
        HtRate::new(
            HtMcs::Mcs2,
            HtGuardInterval::Long800Ns,
            HtChannelWidth::Mhz20,
        )
        .vendor_control_rate(),
        LegacyRate::Ofdm12M,
    );
}

#[test]
fn ht_duplicate_rate_stays_outside_the_ordinary_phy_and_formatter_domains() {
    let duplicate = HtDuplicateRate::new(HtGuardInterval::Short400Ns);
    assert_eq!(duplicate.mcs_index(), 32);
    assert_eq!(duplicate.channel_width(), HtChannelWidth::Mhz40);
    assert_eq!(duplicate.nominal_kbps(), 6_700);

    // The finite ordinary decoder has no raw byte which can manufacture the
    // separate duplicate-mode type.
    for code in u8::MIN..=u8::MAX {
        if let Some(TxPhyRate::Ht(rate)) = TxPhyRate::from_code(code, HtChannelWidth::Mhz40) {
            assert!(rate.mcs.index() <= HtMcs::Mcs7.index());
        }
    }
}

#[test]
fn ht_duplicate_selector_validates_protocol_before_reporting_exact_oracle_gaps() {
    let request = HtDuplicateCertificationRequest::new(
        HtChannelWidth::Mhz40,
        HtGuardInterval::Short400Ns,
        5_484,
    );
    let capable = HtDuplicateTxLinkCapabilities::new(Some(HtChannelWidth::Mhz40), true, true);
    assert_eq!(capable.channel_width(), Some(HtChannelWidth::Mhz40));
    assert!(capable.peer_supports_mcs32());
    assert!(capable.peer_supports_short_guard_interval());

    let selection = select_esp32s31_ht_duplicate_tx(Some(request), capable);
    assert_eq!(selection.request(), Some(request));
    assert_eq!(selection.plan(), None);
    let Some(HtDuplicateTxRejection::Hardware(
        HtDuplicateTxUnavailable::Esp32s31EvidenceIncomplete(evidence),
    )) = selection.rejection()
    else {
        panic!("a protocol-valid request must stop at the reviewed hardware frontier");
    };
    assert_eq!(evidence, HtDuplicateTxEvidenceGaps::ESP32S31);
    let formatter = evidence.formatter();
    assert_eq!(formatter, HtDuplicateTxOracleGaps::ESP32S31);
    assert!(!formatter.is_empty());
    for field in [
        HtDuplicateTxOracleField::DescriptorSelector,
        HtDuplicateTxOracleField::PlcpAndHtSig,
        HtDuplicateTxOracleField::Length,
        HtDuplicateTxOracleField::Protection,
        HtDuplicateTxOracleField::Power,
        HtDuplicateTxOracleField::Retry,
    ] {
        assert!(formatter.contains(field));
    }
    let qualification = evidence.qualification();
    assert_eq!(qualification, HtDuplicateTxQualificationGaps::ESP32S31);
    assert!(!qualification.is_empty());
    assert!(qualification.contains(HtDuplicateTxQualificationField::OnAirAck));
}

#[test]
fn ht_duplicate_selector_reports_each_pre_hardware_rejection_without_a_plan() {
    let request = |width, guard_interval, duration| {
        Some(HtDuplicateCertificationRequest::new(
            width,
            guard_interval,
            duration,
        ))
    };
    let capable = HtDuplicateTxLinkCapabilities::new(Some(HtChannelWidth::Mhz40), true, true);
    assert_eq!(
        select_esp32s31_ht_duplicate_tx(None, capable),
        HtDuplicateTxSelection::NotRequested
    );

    let cases = [
        (
            request(HtChannelWidth::Mhz40, HtGuardInterval::Long800Ns, 0),
            capable,
            HtDuplicateTxRejection::ZeroMaximumPpduDuration,
        ),
        (
            request(HtChannelWidth::Mhz20, HtGuardInterval::Long800Ns, 1),
            capable,
            HtDuplicateTxRejection::RequestedWidthMustBe40Mhz,
        ),
        (
            request(HtChannelWidth::Mhz40, HtGuardInterval::Long800Ns, 1),
            HtDuplicateTxLinkCapabilities::new(Some(HtChannelWidth::Mhz20), true, true),
            HtDuplicateTxRejection::LinkIsNot40Mhz,
        ),
        (
            request(HtChannelWidth::Mhz40, HtGuardInterval::Long800Ns, 1),
            HtDuplicateTxLinkCapabilities::new(Some(HtChannelWidth::Mhz40), false, true),
            HtDuplicateTxRejection::PeerDoesNotSupportMcs32,
        ),
        (
            request(HtChannelWidth::Mhz40, HtGuardInterval::Short400Ns, 1),
            HtDuplicateTxLinkCapabilities::new(Some(HtChannelWidth::Mhz40), true, false),
            HtDuplicateTxRejection::PeerDoesNotSupportShortGuardInterval,
        ),
    ];
    for (request, link, expected) in cases {
        let selection = select_esp32s31_ht_duplicate_tx(request, link);
        assert_eq!(selection.rejection(), Some(expected));
        assert_eq!(selection.plan(), None);
    }
}

#[test]
fn he_retry_rates_follow_the_owned_dot11ax_schedule_and_preserve_ldpc() {
    let mcs9 = HeRate::ldpc(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf1600Ns);
    assert_eq!(mcs9.vendor_retry_rate(0), Some(TxPhyRate::He(mcs9)));
    assert_eq!(mcs9.vendor_retry_rate(1), Some(TxPhyRate::He(mcs9)));
    assert_eq!(
        mcs9.vendor_retry_rate(2),
        Some(TxPhyRate::He(HeRate::ldpc(
            HeMcs::Mcs7,
            HeGuardIntervalAndLtf::TwoLtf1600Ns,
        )))
    );
    assert_eq!(
        mcs9.vendor_retry_rate(4),
        Some(TxPhyRate::Legacy(LegacyRate::Ofdm6M))
    );

    let mcs9_800 = HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::OneLtf800Ns);
    assert_eq!(
        mcs9_800.vendor_retry_rate(2),
        Some(TxPhyRate::He(HeRate::new(
            HeMcs::Mcs8,
            HeGuardIntervalAndLtf::OneLtf800Ns,
        )))
    );
    assert_eq!(
        HeRate::new(HeMcs::Mcs8, HeGuardIntervalAndLtf::OneLtf800Ns).vendor_retry_rate(0),
        None
    );
}

#[test]
fn he_rate_control_codes_round_trip_through_the_dot11ax_arena() {
    for index in 0..=9 {
        let mcs = HeMcs::from_index(index).unwrap();
        for gi_ltf in [
            HeGuardIntervalAndLtf::TwoLtf1600Ns,
            HeGuardIntervalAndLtf::OneLtf800Ns,
            HeGuardIntervalAndLtf::TwoLtf800Ns,
        ] {
            let rate = TxPhyRate::He(HeRate::new(mcs, gi_ltf));
            let code = rate.rate_control_code().unwrap();
            assert_eq!(
                TxPhyRate::from_rate_control_code(
                    RateScheduleKind::Dot11Ax,
                    code,
                    HtChannelWidth::Mhz20,
                    gi_ltf,
                ),
                Some(rate)
            );
        }
    }
    let long = HeRate::new(HeMcs::Mcs0, HeGuardIntervalAndLtf::FourLtf3200Ns);
    assert_eq!(TxPhyRate::He(long).rate_control_code(), None);
}

#[test]
fn rate_control_code_is_decoded_in_its_ht_or_he_arena() {
    let ht = TxPhyRate::from_rate_control_code(
        RateScheduleKind::Dot11N,
        0x17,
        HtChannelWidth::Mhz40,
        HeGuardIntervalAndLtf::OneLtf800Ns,
    );
    assert_eq!(
        ht,
        Some(TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs7,
            HtGuardInterval::Long800Ns,
            HtChannelWidth::Mhz40,
        )))
    );

    let he_long = TxPhyRate::from_rate_control_schedule(
        RateScheduleRef::new(RateScheduleKind::Dot11Ax, 1).unwrap(),
        HtChannelWidth::Mhz20,
        HeGuardIntervalAndLtf::OneLtf800Ns,
    );
    assert_eq!(
        he_long,
        Some(TxPhyRate::He(HeRate::new(
            HeMcs::Mcs9,
            HeGuardIntervalAndLtf::TwoLtf1600Ns,
        )))
    );

    let he_short = TxPhyRate::from_rate_control_schedule(
        RateScheduleRef::new(RateScheduleKind::Dot11Ax, 0).unwrap(),
        HtChannelWidth::Mhz20,
        HeGuardIntervalAndLtf::OneLtf800Ns,
    );
    assert_eq!(
        he_short,
        Some(TxPhyRate::He(HeRate::new(
            HeMcs::Mcs9,
            HeGuardIntervalAndLtf::OneLtf800Ns,
        )))
    );
    assert_eq!(
        TxPhyRate::from_rate_control_code(
            RateScheduleKind::Dot11Ax,
            0x23,
            HtChannelWidth::Mhz20,
            HeGuardIntervalAndLtf::FourLtf3200Ns,
        ),
        None,
    );
}

#[test]
fn he_bcc_dcm_rates_publish_the_recovered_a1_bit_and_ru242_rates() {
    for (mcs, expected_index, expected_kbps) in [
        (HeBccDcmMcs::Mcs0, 0, 4_300),
        (HeBccDcmMcs::Mcs1, 1, 8_600),
        (HeBccDcmMcs::Mcs3, 3, 17_200),
    ] {
        let rate = HeRate::bcc_dcm(mcs, HeGuardIntervalAndLtf::TwoLtf800Ns);
        assert!(rate.is_dcm());
        assert_eq!(rate.mcs().index(), expected_index);
        assert_eq!(rate.code(), 0x1a + expected_index);
        assert_eq!(
            rate.rate_control_dcm_fallback_code(),
            Some(0x10 + expected_index)
        );
        assert_eq!(rate.power_lookup_code(), 0x10 + expected_index);
        assert_eq!(rate.nominal_kbps(), expected_kbps);
        assert_eq!(
            rate.minimum_ampdu_subframe_bytes(HtAmpduDensity::EightMicroseconds),
            expected_kbps.div_ceil(1_000) as u16
        );
    }

    assert_eq!(
        HeRate::bcc_dcm(HeBccDcmMcs::Mcs3, HeGuardIntervalAndLtf::TwoLtf1600Ns).nominal_kbps(),
        16_300
    );
    assert_eq!(
        HeRate::bcc_dcm(HeBccDcmMcs::Mcs3, HeGuardIntervalAndLtf::FourLtf3200Ns).nominal_kbps(),
        14_600
    );
    // Preserve the blob's two-stage integer truncation instead of replacing
    // it with a superficially equivalent ceil(rate*density/80).
    assert_eq!(
        HeRate::bcc_dcm(HeBccDcmMcs::Mcs1, HeGuardIntervalAndLtf::TwoLtf1600Ns)
            .minimum_ampdu_subframe_bytes(HtAmpduDensity::QuarterMicrosecond),
        1
    );
}

#[test]
fn he_ldpc_profile_owns_coding_control_and_the_dcm_mcs4_rom_column() {
    let ordinary = HeRate::ldpc(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns);
    assert_eq!(ordinary.fec_coding(), HeFecCoding::Ldpc);
    assert!(ordinary.is_ldpc());
    assert!(!ordinary.is_dcm());
    for (gi_ltf, expected_kbps) in [
        (HeGuardIntervalAndLtf::TwoLtf800Ns, 25_800),
        (HeGuardIntervalAndLtf::TwoLtf1600Ns, 24_400),
        (HeGuardIntervalAndLtf::FourLtf3200Ns, 21_900),
    ] {
        let rate = HeRate::ldpc_dcm(HeLdpcDcmMcs::Mcs4, gi_ltf);
        assert_eq!(rate.fec_coding(), HeFecCoding::Ldpc);
        assert!(rate.is_dcm());
        assert_eq!(rate.mcs(), HeMcs::Mcs4);
        assert_eq!(rate.code(), 0x1e);
        // rcGetDCMMaxRate publishes only its separate MCS0/1/3 fallback
        // domain. Direct LDPC+DCM MCS4 retains the canonical HE rate code.
        assert_eq!(rate.rate_control_dcm_fallback_code(), None);
        assert_eq!(rate.nominal_kbps(), expected_kbps);
    }
}

#[test]
fn he_resource_unit_rates_match_all_complete_blob_table_endpoints() {
    let mcs0 = HeRate::new(HeMcs::Mcs0, HeGuardIntervalAndLtf::TwoLtf800Ns);
    let mcs9 = HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns);
    for (ru, mcs0_kbps, mcs9_kbps) in [
        (HeResourceUnit::Ru26, 900, 11_800),
        (HeResourceUnit::Ru52, 1_800, 23_500),
        (HeResourceUnit::Ru106, 3_800, 50_000),
        (HeResourceUnit::Ru242, 8_600, 114_700),
    ] {
        assert_eq!(mcs0.nominal_kbps_for_resource_unit(ru), mcs0_kbps);
        assert_eq!(mcs9.nominal_kbps_for_resource_unit(ru), mcs9_kbps);
    }
    assert_eq!(mcs9.nominal_kbps(), 114_700);

    let dcm_mcs3 = HeRate::bcc_dcm(HeBccDcmMcs::Mcs3, HeGuardIntervalAndLtf::FourLtf3200Ns);
    let dcm_mcs4 = HeRate::ldpc_dcm(HeLdpcDcmMcs::Mcs4, HeGuardIntervalAndLtf::TwoLtf1600Ns);
    for (ru, mcs3_kbps, mcs4_kbps) in [
        (HeResourceUnit::Ru26, 1_500, 2_500),
        (HeResourceUnit::Ru52, 3_000, 5_000),
        (HeResourceUnit::Ru106, 6_400, 10_600),
        (HeResourceUnit::Ru242, 14_600, 24_400),
    ] {
        assert_eq!(dcm_mcs3.nominal_kbps_for_resource_unit(ru), mcs3_kbps);
        assert_eq!(dcm_mcs4.nominal_kbps_for_resource_unit(ru), mcs4_kbps);
    }
}

fn scheduled_trigger_user(
    aid12: u16,
    ru_allocation: u8,
    coding_type: bool,
    mcs: u8,
    dcm: bool,
    starting_spatial_stream_encoding: u8,
    spatial_stream_count_encoding: u8,
) -> [u8; 5] {
    [
        aid12 as u8,
        ((aid12 >> 8) as u8 & 0x0f) | ((ru_allocation & 0x07) << 5),
        ((ru_allocation >> 3) & 0x0f) | ((coding_type as u8) << 4) | ((mcs & 0x07) << 5),
        ((mcs >> 3) & 0x01)
            | ((dcm as u8) << 1)
            | ((starting_spatial_stream_encoding & 0x07) << 2)
            | ((spatial_stream_count_encoding & 0x07) << 5),
        0x7f,
    ]
}

fn basic_trigger_with_users(users: &[[u8; 5]]) -> Vec<u8> {
    let mut frame = vec![0_u8; 24];
    frame[..2].copy_from_slice(&0x0024_u16.to_le_bytes());
    // Trigger Common Info selector one is 2x HE-LTF + 1.6-us GI. This is a
    // different wire table from HE-SU HE-SIG-A GI/LTF.
    frame[16..24].copy_from_slice(&(1_u64 << 20).to_le_bytes());
    for user in users {
        frame.extend_from_slice(user);
        frame.push(0);
    }
    frame
}

#[test]
fn scheduled_he20_trigger_rate_selects_our_user_from_the_complete_iterator() {
    let other = scheduled_trigger_user(0x123, 0, false, 0, false, 0, 0);
    let assigned = scheduled_trigger_user(0x234, 53, true, 4, true, 0, 0);
    let bytes = basic_trigger_with_users(&[other, assigned]);
    let frame = parse_trigger_frame(&bytes).unwrap();
    let scheduled = HeTriggerScheduledRate::from_trigger_frame(&frame, 0x234).unwrap();
    assert_eq!(scheduled.resource_unit, HeResourceUnit::Ru106);
    assert_eq!(scheduled.resource_unit_index, 1);
    assert_eq!(scheduled.rate.mcs(), HeMcs::Mcs4);
    assert!(scheduled.rate.is_ldpc());
    assert!(scheduled.rate.is_dcm());

    assert_eq!(
        HeTriggerScheduledRate::from_trigger_frame(&frame, 0x345),
        Err(HeTriggerScheduledRateError::AssociationIdNotScheduled)
    );

    let duplicate_bytes = basic_trigger_with_users(&[assigned, assigned]);
    let duplicate = parse_trigger_frame(&duplicate_bytes).unwrap();
    assert_eq!(
        HeTriggerScheduledRate::from_trigger_frame(&duplicate, 0x234),
        Err(HeTriggerScheduledRateError::DuplicateAssociationId)
    );

    let mut malformed_bytes = basic_trigger_with_users(&[assigned]);
    malformed_bytes.push(0);
    let malformed = parse_trigger_frame(&malformed_bytes).unwrap();
    assert!(matches!(
        HeTriggerScheduledRate::from_trigger_frame(&malformed, 0x234),
        Err(HeTriggerScheduledRateError::MalformedUserInfo(_))
    ));

    let mut padding_hidden_bytes = basic_trigger_with_users(&[]);
    padding_hidden_bytes.extend_from_slice(&[0xff, 0xef, 0x0f, 0, 0]);
    padding_hidden_bytes.extend_from_slice(&assigned);
    padding_hidden_bytes.push(0);
    let padding_hidden = parse_trigger_frame(&padding_hidden_bytes).unwrap();
    assert_eq!(
        HeTriggerScheduledRate::from_trigger_frame(&padding_hidden, 0x234),
        Err(HeTriggerScheduledRateError::AssociationIdNotScheduled)
    );
}

#[test]
fn scheduled_he20_trigger_rate_fails_closed_at_every_owned_boundary() {
    let common =
        parse_trigger_common_info(&(1_u64 << 20).to_le_bytes()).expect("complete common info");
    let user_bytes = scheduled_trigger_user(0x234, 53, true, 4, true, 0, 0);
    let user = parse_trigger_user_spatial_stream(&user_bytes).unwrap();
    let scheduled = HeTriggerScheduledRate::new(common, user, 0x234).unwrap();
    assert_eq!(scheduled.resource_unit, HeResourceUnit::Ru106);
    assert_eq!(scheduled.resource_unit_index, 1);
    assert_eq!(scheduled.partial_ru_power_selector.trigger_encoding(), 53);
    assert_eq!(scheduled.rate.mcs(), HeMcs::Mcs4);
    assert_eq!(
        scheduled.trigger_gi_ltf,
        oer_ieee80211_mac::trigger::TriggerGiLtf::TwoLtf1600Ns
    );
    assert!(scheduled.rate.is_ldpc());
    assert!(scheduled.rate.is_dcm());
    assert_eq!(scheduled.nominal_kbps(), 10_600);

    let bsrp_common = parse_trigger_common_info(&((2_u64 << 20) | 4).to_le_bytes()).unwrap();
    assert_eq!(
        HeTriggerScheduledRate::new(bsrp_common, user, 0x234),
        Err(HeTriggerScheduledRateError::UnsupportedTriggerType)
    );
    let wide_common =
        parse_trigger_common_info(&((2_u64 << 20) | (1 << 18)).to_le_bytes()).unwrap();
    assert_eq!(
        HeTriggerScheduledRate::new(wide_common, user, 0x234),
        Err(HeTriggerScheduledRateError::UnsupportedBandwidth)
    );
    assert_eq!(
        HeTriggerScheduledRate::new(common, user, 0x235),
        Err(HeTriggerScheduledRateError::AssociationIdMismatch)
    );

    let two_stream_bytes = scheduled_trigger_user(0x234, 53, true, 4, true, 0, 1);
    let two_streams = parse_trigger_user_spatial_stream(&two_stream_bytes).unwrap();
    assert_eq!(
        HeTriggerScheduledRate::new(common, two_streams, 0x234),
        Err(HeTriggerScheduledRateError::UnsupportedSpatialStreams)
    );

    for ru_allocation in [9, 62, 69] {
        let unsupported_ru_bytes =
            scheduled_trigger_user(0x234, ru_allocation, false, 0, false, 0, 0);
        let unsupported_ru = parse_trigger_user_spatial_stream(&unsupported_ru_bytes).unwrap();
        assert_eq!(
            HeTriggerScheduledRate::new(common, unsupported_ru, 0x234),
            Err(HeTriggerScheduledRateError::UnsupportedResourceUnit)
        );
    }

    let mcs10_bytes = scheduled_trigger_user(0x234, 53, true, 10, false, 0, 0);
    let mcs10 = parse_trigger_user_spatial_stream(&mcs10_bytes).unwrap();
    assert_eq!(
        HeTriggerScheduledRate::new(common, mcs10, 0x234),
        Err(HeTriggerScheduledRateError::UnsupportedMcs)
    );

    let dcm_mcs2_bytes = scheduled_trigger_user(0x234, 53, true, 2, true, 0, 0);
    let dcm_mcs2 = parse_trigger_user_spatial_stream(&dcm_mcs2_bytes).unwrap();
    assert_eq!(
        HeTriggerScheduledRate::new(common, dcm_mcs2, 0x234),
        Err(HeTriggerScheduledRateError::UnsupportedDcmCombination)
    );

    let reserved_gi =
        parse_trigger_common_info(&(3_u64 << 20).to_le_bytes()).expect("complete common info");
    assert_eq!(
        HeTriggerScheduledRate::new(reserved_gi, user, 0x234),
        Err(HeTriggerScheduledRateError::UnsupportedGiLtf)
    );
}

#[test]
fn legacy_rts_rates_match_the_complete_vendor_selector() {
    let cases = [
        (LegacyRate::Dsss1MLong, LegacyRate::Dsss1MLong),
        (LegacyRate::Dsss2MLong, LegacyRate::Dsss2MLong),
        (LegacyRate::Cck5M5Long, LegacyRate::Dsss2MLong),
        (LegacyRate::Cck11MLong, LegacyRate::Dsss2MLong),
        (LegacyRate::Dsss2MShort, LegacyRate::Dsss2MShort),
        (LegacyRate::Cck5M5Short, LegacyRate::Dsss2MShort),
        (LegacyRate::Cck11MShort, LegacyRate::Dsss2MShort),
        (LegacyRate::Ofdm48M, LegacyRate::Ofdm24M),
        (LegacyRate::Ofdm24M, LegacyRate::Ofdm24M),
        (LegacyRate::Ofdm12M, LegacyRate::Ofdm12M),
        (LegacyRate::Ofdm6M, LegacyRate::Ofdm6M),
        (LegacyRate::Ofdm54M, LegacyRate::Ofdm24M),
        (LegacyRate::Ofdm36M, LegacyRate::Ofdm24M),
        (LegacyRate::Ofdm18M, LegacyRate::Ofdm12M),
        (LegacyRate::Ofdm9M, LegacyRate::Ofdm6M),
    ];
    for (data, expected) in cases {
        assert_eq!(data.vendor_control_rate(), expected);
    }
}

#[test]
fn data_queue_priorities_match_the_complete_blob_event_mapping() {
    for (queue, expected) in [
        (LegacyTxQueue::Voice, 3),
        (LegacyTxQueue::Video, 2),
        (LegacyTxQueue::BestEffort, 1),
        (LegacyTxQueue::Background, 1),
    ] {
        assert_eq!(queue.vendor_data_packet_priority(), expected);
        assert_eq!(queue.vendor_data_scheduler_priority(), expected);
    }
}

#[test]
fn management_profile_derives_plcp1_from_mpdu_plus_fcs() {
    let config = LegacyTxConfig::management_1m_from_mpdu_length(30).unwrap();
    assert_eq!(config.signal, 0x22);
    assert!(LegacyTxConfig::management_1m_from_mpdu_length(0x0ffc).is_none());
}
