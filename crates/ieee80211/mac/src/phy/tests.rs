use super::*;

#[test]
fn mpdu_spacing_rounds_exact_symbol_rates_up_to_octets() {
    let ht = PhyRate::Ht(HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, true).unwrap());
    // 260 bits / 3.6 us, multiplied by each spacing then divided by eight.
    for (encoding, bytes) in [0, 3, 5, 10, 19, 37, 73, 145].into_iter().enumerate() {
        assert_eq!(ht.minimum_mpdu_spacing_bytes(encoding as u8), Some(bytes));
    }
    assert_eq!(ht.minimum_mpdu_spacing_bytes(8), None);
    assert_eq!(
        PhyRate::Legacy(LegacyRate::Ofdm54M).minimum_mpdu_spacing_bytes(0),
        None
    );
    let he = PhyRate::He(
        HeRate::new(
            HeMcs::new(11).unwrap(),
            SpatialStreams::new(8).unwrap(),
            PpduBandwidth::Mhz160,
            HeGiLtf::Ltf2xGi800Ns,
            FecCoding::Ldpc,
            false,
        )
        .unwrap(),
    );
    // 1960 subcarriers * 10 bits * 5/6 * 8 streams, 13.6 us symbols.
    assert_eq!(he.minimum_mpdu_spacing_bytes(7), Some(19_216));
}

#[test]
fn ht_rates_exist_only_at_20_and_40_mhz() {
    let mcs = HtMcs::new(7).unwrap();
    assert!(HtRate::new(mcs, PpduBandwidth::Mhz40, true).is_some());
    assert!(HtRate::new(mcs, PpduBandwidth::Mhz80, false).is_none());
    assert_eq!(HtMcs::new(31).unwrap().spatial_streams(), 4);
    assert_eq!(HtMcs::new(32), None);
}

#[test]
fn he_dcm_is_limited_to_its_standard_mcs_and_streams() {
    let rate = |mcs, streams, dcm| {
        HeRate::new(
            HeMcs::new(mcs).unwrap(),
            SpatialStreams::new(streams).unwrap(),
            PpduBandwidth::Mhz20,
            HeGiLtf::Ltf2xGi800Ns,
            FecCoding::Bcc,
            dcm,
        )
    };
    for mcs in 0..=11 {
        assert!(rate(mcs, 1, false).is_some());
        assert_eq!(rate(mcs, 1, true).is_some(), matches!(mcs, 0 | 1 | 3 | 4));
    }
    assert!(rate(0, 2, true).is_some());
    assert!(rate(0, 3, true).is_none());
    assert_eq!(HeMcs::new(12), None);
    assert_eq!(SpatialStreams::new(0), None);
    assert_eq!(SpatialStreams::new(9), None);
}

#[test]
fn legacy_rates_report_their_modulation_and_rate() {
    assert!(!LegacyRate::Cck11M(DsssPreamble::Short).is_ofdm());
    assert!(LegacyRate::Ofdm6M.is_ofdm());
    assert_eq!(LegacyRate::Cck5M5(DsssPreamble::Long).kbps(), 5_500);
    assert_eq!(
        PhyRate::Legacy(LegacyRate::Ofdm54M).bandwidth(),
        PpduBandwidth::Mhz20
    );
}

#[test]
fn nominal_rates_follow_the_standard_tables() {
    let ht = |mcs, bandwidth, short_gi| {
        PhyRate::Ht(HtRate::new(HtMcs::new(mcs).unwrap(), bandwidth, short_gi).unwrap())
    };
    assert_eq!(ht(0, PpduBandwidth::Mhz20, false).nominal_kbps(), 6_500);
    assert_eq!(ht(7, PpduBandwidth::Mhz20, true).nominal_kbps(), 72_200);
    assert_eq!(ht(7, PpduBandwidth::Mhz40, false).nominal_kbps(), 135_000);
    assert_eq!(ht(15, PpduBandwidth::Mhz40, true).nominal_kbps(), 300_000);

    let he = |mcs, gi_ltf, dcm| {
        PhyRate::He(
            HeRate::new(
                HeMcs::new(mcs).unwrap(),
                SpatialStreams::ONE,
                PpduBandwidth::Mhz20,
                gi_ltf,
                FecCoding::Bcc,
                dcm,
            )
            .unwrap(),
        )
    };
    // 117 bits per 13.6 µs symbol.
    assert_eq!(he(0, HeGiLtf::Ltf2xGi800Ns, false).nominal_kbps(), 8_602);
    assert_eq!(he(9, HeGiLtf::Ltf2xGi800Ns, false).nominal_kbps(), 114_705);
    assert_eq!(
        he(11, HeGiLtf::Ltf4xGi3200Ns, false).nominal_kbps(),
        121_875
    );
    assert_eq!(he(1, HeGiLtf::Ltf2xGi1600Ns, true).nominal_kbps(), 8_125);
    assert_eq!(
        PhyRate::Legacy(LegacyRate::Cck5M5(DsssPreamble::Short)).nominal_kbps(),
        5_500
    );
}

#[test]
fn ppdu_durations_follow_the_txtime_of_each_format() {
    // ERP-OFDM: 20 us preamble, 4 us symbols of 24 bits at 6 Mb/s, and the
    // signal extension. A 32-octet BlockAck: (16 + 256 + 6) / 24 -> 12.
    assert_eq!(
        PhyRate::Legacy(LegacyRate::Ofdm6M).max_ppdu_duration_micros(32),
        20 + 48 + 6
    );
    // DSSS 1 Mb/s with the long preamble: 192 us and 8 us an octet.
    assert_eq!(
        PhyRate::Legacy(LegacyRate::Dsss1M).max_ppdu_duration_micros(14),
        192 + 112
    );
    // HT20 MCS 7, long GI, 1500 octets: 260 bits a symbol,
    // (16 + 12000 + 6) / 260 -> 47 symbols after the 36 us preamble.
    let ht = HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, false).unwrap();
    assert_eq!(
        PhyRate::Ht(ht).max_ppdu_duration_micros(1_500),
        36 + 47 * 4 + 6
    );
    // The short GI keeps the symbols and shortens them to 3.6 us.
    let short = HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, true).unwrap();
    assert_eq!(
        PhyRate::Ht(short).max_ppdu_duration_micros(1_500),
        36 + 4 * 43 + 6
    );
    // HT40 doubles the bits a symbol.
    let ht40 = HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz40, false).unwrap();
    assert!(
        PhyRate::Ht(ht40).max_ppdu_duration_micros(1_500)
            < PhyRate::Ht(ht).max_ppdu_duration_micros(1_500)
    );
    // HE20 MCS 7, one stream, 2x LTF and 0.8 us GI: 234 * 6 * 5/6 = 1170
    // bits a 13.6 us symbol, (16 + 12000 + 6) / 1170 -> 11 symbols.
    let he = HeRate::new(
        HeMcs::new(7).unwrap(),
        SpatialStreams::new(1).unwrap(),
        PpduBandwidth::Mhz20,
        HeGiLtf::Ltf2xGi800Ns,
        FecCoding::Bcc,
        false,
    )
    .unwrap();
    assert_eq!(
        PhyRate::He(he).max_ppdu_duration_micros(1_500),
        (36_000_u32 + 7_200 + 11 * 13_600).div_ceil(1_000) + 16 + 6
    );
    // Longer PSDUs never take less time.
    for rate in [
        PhyRate::Ht(ht),
        PhyRate::He(he),
        PhyRate::Legacy(LegacyRate::Ofdm54M),
    ] {
        assert!(rate.max_ppdu_duration_micros(3_000) >= rate.max_ppdu_duration_micros(1_500));
    }
}
