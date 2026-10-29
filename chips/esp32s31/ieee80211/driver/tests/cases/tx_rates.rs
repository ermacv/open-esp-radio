use crate::*;

#[test]
fn legacy_tx_preserves_the_nominal_bitrate() {
    assert_eq!(
        TxPhyRate::Legacy(MacLegacyRate::Ofdm54M).nominal_kbps(),
        54_000
    );
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
    assert_eq!(sgi.vendor_control_rate(), MacLegacyRate::Ofdm24M);

    assert_eq!(
        HtRate::new(
            HtMcs::Mcs0,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz20,
        )
        .vendor_control_rate(),
        MacLegacyRate::Ofdm6M,
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
        MacLegacyRate::Ofdm12M,
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

    let he_long = TxPhyRate::from_rate_control_code(
        RateScheduleKind::Dot11Ax,
        oer_espressif_ieee80211_policy::rate_schedule::schedule_state(
            oer_espressif_ieee80211_policy::rate_schedule::rate_schedule(
                RateScheduleKind::Dot11Ax,
                1,
            )
            .unwrap(),
        )
        .rate,
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

    let he_short = TxPhyRate::from_rate_control_code(
        RateScheduleKind::Dot11Ax,
        oer_espressif_ieee80211_policy::rate_schedule::schedule_state(
            oer_espressif_ieee80211_policy::rate_schedule::rate_schedule(
                RateScheduleKind::Dot11Ax,
                0,
            )
            .unwrap(),
        )
        .rate,
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

#[test]
fn legacy_rts_rates_match_the_complete_vendor_selector() {
    let cases = [
        (MacLegacyRate::Dsss1MLong, MacLegacyRate::Dsss1MLong),
        (MacLegacyRate::Dsss2MLong, MacLegacyRate::Dsss2MLong),
        (MacLegacyRate::Cck5M5Long, MacLegacyRate::Dsss2MLong),
        (MacLegacyRate::Cck11MLong, MacLegacyRate::Dsss2MLong),
        (MacLegacyRate::Dsss2MShort, MacLegacyRate::Dsss2MShort),
        (MacLegacyRate::Cck5M5Short, MacLegacyRate::Dsss2MShort),
        (MacLegacyRate::Cck11MShort, MacLegacyRate::Dsss2MShort),
        (MacLegacyRate::Ofdm48M, MacLegacyRate::Ofdm24M),
        (MacLegacyRate::Ofdm24M, MacLegacyRate::Ofdm24M),
        (MacLegacyRate::Ofdm12M, MacLegacyRate::Ofdm12M),
        (MacLegacyRate::Ofdm6M, MacLegacyRate::Ofdm6M),
        (MacLegacyRate::Ofdm54M, MacLegacyRate::Ofdm24M),
        (MacLegacyRate::Ofdm36M, MacLegacyRate::Ofdm24M),
        (MacLegacyRate::Ofdm18M, MacLegacyRate::Ofdm12M),
        (MacLegacyRate::Ofdm9M, MacLegacyRate::Ofdm6M),
    ];
    for (data, expected) in cases {
        assert_eq!(TxPhyRate::Legacy(data).vendor_control_rate(), expected);
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
    let config = LegacyTxConfig::management_from_mpdu_length(MacLegacyRate::Ofdm6M, 30).unwrap();
    assert_eq!(config.signal, 0x22);
    assert!(LegacyTxConfig::management_from_mpdu_length(MacLegacyRate::Ofdm6M, 0x0ffc).is_none());
}
