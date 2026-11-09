use super::*;

fn nonzero(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value).unwrap()
}

/// HE20 MCS0 (BPSK, R = 1/2) on a 242-tone RU with 1x HE-LTF and 0.8-us GI.
fn mcs0(coding: HeSuCoding, padding: HeNominalPacketPadding) -> HeSuTiming {
    HeSuTiming {
        data_bits_per_symbol: nonzero(117),
        coded_bits_per_symbol: nonzero(234),
        short_data_bits_per_symbol: nonzero(30),
        short_coded_bits_per_symbol: nonzero(60),
        coding,
        symbol_nanos: nonzero(13_600),
        preamble_nanos: 40_000,
        nominal_packet_padding: padding,
        signal_extension_micros: 6,
    }
}

/// HE20 MCS7 (64-QAM, R = 5/6), 1x HE-LTF and 0.8-us GI.
fn mcs7(coding: HeSuCoding) -> HeSuTiming {
    HeSuTiming {
        data_bits_per_symbol: nonzero(1170),
        coded_bits_per_symbol: nonzero(1404),
        short_data_bits_per_symbol: nonzero(300),
        short_coded_bits_per_symbol: nonzero(360),
        ..mcs0(coding, HeNominalPacketPadding::Micros16)
    }
}

/// HE20 MCS9 (256-QAM, R = 5/6), 4x HE-LTF and 3.2-us GI.
fn mcs9(coding: HeSuCoding) -> HeSuTiming {
    HeSuTiming {
        data_bits_per_symbol: nonzero(1560),
        coded_bits_per_symbol: nonzero(1872),
        short_data_bits_per_symbol: nonzero(400),
        short_coded_bits_per_symbol: nonzero(480),
        coding,
        symbol_nanos: nonzero(16_000),
        preamble_nanos: 52_000,
        nominal_packet_padding: HeNominalPacketPadding::Micros16,
        signal_extension_micros: 6,
    }
}

const BCC: HeSuCoding = HeSuCoding::Bcc;

fn symbols(symbols: u64, padding_factor: u64) -> HeSuDataSymbols {
    HeSuDataSymbols {
        symbols,
        padding_factor,
    }
}

#[test]
fn bcc_padding_factor_counts_quarter_symbols_of_the_excess() {
    let timing = mcs0(BCC, HeNominalPacketPadding::Micros16);
    // 1500 bytes: 12000 + 6 + 16 = 12022 bits = 102 x 117 + 88, so 103
    // symbols; ceil(88 / 30) = 3 quarters.
    assert_eq!(timing.data_symbols(1500), symbols(103, 3));
    // 40 + 103 x 13.6 + 4 x 3 + 6 = 1458.8 us.
    assert_eq!(timing.duration_micros(1500), 1459);
    // 85 bytes: 702 bits = 6 x 117 exactly: no excess, a = 4.
    assert_eq!(timing.data_symbols(85), symbols(6, 4));
    // 84 bytes: 694 bits, excess 109 > 3 x 30: still a = 4.
    assert_eq!(timing.data_symbols(84), symbols(6, 4));
    // 86 bytes: 710 bits, one more symbol with an 8-bit excess, a = 1.
    assert_eq!(timing.data_symbols(86), symbols(7, 1));
}

#[test]
fn a_new_symbol_outweighs_the_packet_extension_it_saves() {
    let timing = mcs0(BCC, HeNominalPacketPadding::Micros16);
    // 40 + 6 x 13.6 + 16 + 6 = 143.6 us.
    assert_eq!(timing.duration_micros(85), 144);
    // 40 + 7 x 13.6 + 4 + 6 = 145.2 us.
    assert_eq!(timing.duration_micros(86), 146);
    // 89 bytes: 734 bits = 6 x 117 + 32, a = 2: 40 + 95.2 + 8 + 6 = 149.2 us.
    assert_eq!(timing.duration_micros(89), 150);
}

#[test]
fn nominal_packet_padding_scales_the_extension_by_padding_factor() {
    // 1500 bytes leave a = 3 (above): T_PE = 12, 4 and 0 us.
    for (padding, micros) in [
        (HeNominalPacketPadding::Micros16, 1459),
        (HeNominalPacketPadding::Micros8, 1451),
        (HeNominalPacketPadding::Micros0, 1447),
    ] {
        assert_eq!(mcs0(BCC, padding).duration_micros(1500), micros);
    }
    // a = 4 (85 bytes): 16, 8 and 0 us; a = 1 (86 bytes): 4, 0 and 0 us.
    assert_eq!(
        mcs0(BCC, HeNominalPacketPadding::Micros8).duration_micros(85),
        136
    );
    assert_eq!(
        mcs0(BCC, HeNominalPacketPadding::Micros8).duration_micros(86),
        142
    );
    assert_eq!(
        mcs0(BCC, HeNominalPacketPadding::Micros0).duration_micros(85),
        128
    );
}

#[test]
fn an_empty_apep_is_an_ndp_with_a_four_microsecond_extension() {
    for padding in [
        HeNominalPacketPadding::Micros0,
        HeNominalPacketPadding::Micros8,
        HeNominalPacketPadding::Micros16,
    ] {
        // 40 + 4 + 6 us: no data field.
        assert_eq!(mcs0(BCC, padding).duration_micros(0), 50);
    }
}

#[test]
fn ldpc_has_no_tail_and_keeps_an_unpunctured_codeword_set() {
    let timing = mcs0(
        HeSuCoding::Ldpc(LdpcCodeRate::Half),
        HeNominalPacketPadding::Micros16,
    );
    // 12016 bits = 102 x 117 + 82, a_init = 3. N_pld = 12024,
    // N_avbits = 24048, N_CW = 13, N_shrt = 612, N_punc = 612, below
    // 0.1 x 13 x 1944 x 1/2 = 1263.6: no extra symbol segment.
    assert_eq!(timing.data_symbols(1500), symbols(103, 3));
    assert_eq!(timing.duration_micros(1500), 1459);
}

#[test]
fn ldpc_extra_segment_raises_the_padding_factor_within_a_symbol() {
    let ldpc = mcs9(HeSuCoding::Ldpc(LdpcCodeRate::FiveSixths));
    // 47 bytes: 392 bits, a_init = 1. N_pld = 400, N_avbits = 480 < 400 +
    // 912/6, so L_LDPC = 648: N_shrt = 140, N_punc = 28 > 10.8 and
    // 140 < 1.2 x 28 x 5 = 168: extra segment, a = 2.
    assert_eq!(ldpc.data_symbols(47), symbols(1, 2));
    // 52 + 16 + 8 + 6 us.
    assert_eq!(ldpc.duration_micros(47), 82);
    // BCC: 398 bits fit one quarter: 52 + 16 + 4 + 6 us.
    assert_eq!(mcs9(BCC).data_symbols(47), symbols(1, 1));
    assert_eq!(mcs9(BCC).duration_micros(47), 78);
}

#[test]
fn ldpc_extra_segment_after_a_full_last_symbol_adds_a_symbol() {
    let ldpc = mcs7(HeSuCoding::Ldpc(LdpcCodeRate::FiveSixths));
    // 140 bytes: 1136 bits, a_init = 4. N_pld = 1170, N_avbits = 1404,
    // L_LDPC = 1944: N_shrt = 450, N_punc = 90 > 32.4 and 450 < 540:
    // extra segment, two symbols with a = 1.
    assert_eq!(ldpc.data_symbols(140), symbols(2, 1));
    // 40 + 2 x 13.6 + 4 + 6 = 77.2 us.
    assert_eq!(ldpc.duration_micros(140), 78);
    // BCC: 1142 bits, one symbol with a = 4: 40 + 13.6 + 16 + 6 = 75.6 us.
    assert_eq!(mcs7(BCC).data_symbols(140), symbols(1, 4));
    assert_eq!(mcs7(BCC).duration_micros(140), 76);
}

#[test]
fn maximum_apep_is_the_last_length_before_the_next_microsecond() {
    let timing = mcs0(BCC, HeNominalPacketPadding::Micros16);
    // One byte takes 40 + 13.6 + 4 + 6 = 63.6 us.
    assert_eq!(timing.maximum_apep_bytes(63), None);
    assert_eq!(timing.maximum_apep_bytes(64), NonZeroU16::new(1));
    assert_eq!(timing.maximum_apep_bytes(144), NonZeroU16::new(85));
    assert_eq!(timing.maximum_apep_bytes(145), NonZeroU16::new(85));
    // Seven symbols with a = 1 hold 6 x 117 + 30 = 732 bits: 88 bytes.
    assert_eq!(timing.maximum_apep_bytes(146), NonZeroU16::new(88));
    assert_eq!(timing.maximum_apep_bytes(149), NonZeroU16::new(88));
    // a = 2 holds 762 bits: 92 bytes.
    assert_eq!(timing.maximum_apep_bytes(150), NonZeroU16::new(92));
}

#[test]
fn inversion_fits_and_the_next_byte_exceeds_each_budget() {
    for timing in [
        mcs0(BCC, HeNominalPacketPadding::Micros16),
        mcs0(BCC, HeNominalPacketPadding::Micros8),
        mcs0(
            HeSuCoding::Ldpc(LdpcCodeRate::Half),
            HeNominalPacketPadding::Micros0,
        ),
        mcs7(BCC),
        mcs7(HeSuCoding::Ldpc(LdpcCodeRate::FiveSixths)),
        mcs9(HeSuCoding::Ldpc(LdpcCodeRate::FiveSixths)),
    ] {
        let mut previous = 0;
        for bytes in 1..=u16::MAX {
            let micros = timing.duration_micros(bytes);
            assert!(micros >= previous, "duration never falls with length");
            previous = micros;
        }
        for micros in 0..3000 {
            match timing.maximum_apep_bytes(micros) {
                Some(bytes) => {
                    assert!(timing.duration_micros(bytes.get()) <= micros);
                    if let Some(next) = bytes.get().checked_add(1) {
                        assert!(timing.duration_micros(next) > micros);
                    }
                }
                None => assert!(timing.duration_micros(1) > micros),
            }
        }
        assert_eq!(
            timing.maximum_apep_bytes(u32::MAX),
            NonZeroU16::new(u16::MAX)
        );
    }
}

#[test]
fn the_largest_apep_stays_within_u32_microseconds() {
    // Even one data bit per symbol of the longest symbol stays finite.
    let timing = HeSuTiming {
        data_bits_per_symbol: nonzero(1),
        coded_bits_per_symbol: nonzero(1),
        short_data_bits_per_symbol: nonzero(1),
        short_coded_bits_per_symbol: nonzero(1),
        coding: HeSuCoding::Ldpc(LdpcCodeRate::Half),
        symbol_nanos: nonzero(u16::MAX),
        preamble_nanos: u16::MAX,
        nominal_packet_padding: HeNominalPacketPadding::Micros16,
        signal_extension_micros: u8::MAX,
    };
    assert!(timing.duration_micros(u16::MAX) < u32::MAX);
}
