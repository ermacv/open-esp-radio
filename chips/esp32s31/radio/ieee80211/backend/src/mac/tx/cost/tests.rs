use super::*;
use crate::mac::tx::{HeMcs, HtMcs};
use crate::mac::tx_cost::HeSuTiming;

fn block_ack() -> PpduTiming {
    TxPhyRate::Legacy(MacLegacyRate::Ofdm24M)
        .ppdu_timing()
        .unwrap()
}

#[test]
fn one_millisecond_grant_changes_with_actual_ht_geometry() {
    let slow = HtRate::new(
        HtMcs::Mcs0,
        HtGuardInterval::Long800Ns,
        HtChannelWidth::Mhz20,
    );
    let fast = HtRate::new(
        HtMcs::Mcs7,
        HtGuardInterval::Short400Ns,
        HtChannelWidth::Mhz40,
    );
    // 24-Mbit/s compressed BA + SIFS: 48 us. The remaining PPDU budget
    // fits 227 HT20 MCS0 symbols or 252 HT40 SGI MCS7 symbols.
    assert_eq!(
        slow.ampdu_exchange_byte_limit(1000, block_ack(), u16::MAX)
            .unwrap()
            .get(),
        735
    );
    assert_eq!(
        fast.ampdu_exchange_byte_limit(1000, block_ack(), u16::MAX)
            .unwrap()
            .get(),
        17007
    );
    assert_eq!(
        fast.ampdu_exchange_byte_limit(1000, block_ack(), 8191)
            .unwrap()
            .get(),
        8191
    );
    assert_eq!(fast.ampdu_exchange_byte_limit(1000, block_ack(), 0), None);
    assert_eq!(
        slow.ampdu_exchange_byte_limit(u32::MAX, block_ack(), u16::MAX),
        slow.vendor_ampdu_byte_limit().and_then(NonZeroU16::new)
    );
}

#[test]
fn every_ht_mode_preserves_airtime_peer_and_vendor_bounds() {
    for mcs in [
        HtMcs::Mcs0,
        HtMcs::Mcs1,
        HtMcs::Mcs2,
        HtMcs::Mcs3,
        HtMcs::Mcs4,
        HtMcs::Mcs5,
        HtMcs::Mcs6,
        HtMcs::Mcs7,
    ] {
        for gi in [HtGuardInterval::Long800Ns, HtGuardInterval::Short400Ns] {
            for width in [HtChannelWidth::Mhz20, HtChannelWidth::Mhz40] {
                let rate = HtRate::new(mcs, gi, width);
                let phy = TxPhyRate::Ht(rate).ppdu_timing().unwrap();
                let cap = rate
                    .vendor_ampdu_byte_limit()
                    .unwrap_or(u16::MAX)
                    .min(32767);
                for micros in [0, 48, 90, 100, 250, 1000, 4000, 10000, u32::MAX] {
                    match rate.ampdu_exchange_byte_limit(micros, block_ack(), 32767) {
                        Some(bytes) => {
                            assert!(bytes.get() <= cap);
                            assert!(phy.duration_micros(bytes.get()) + 48 <= micros);
                            if bytes.get() < cap {
                                assert!(phy.duration_micros(bytes.get() + 1) + 48 > micros);
                            }
                        }
                        None => assert!(phy.duration_micros(1) + 48 > micros),
                    }
                }
            }
        }
    }
}

const HE_GI_LTF: [HeGuardIntervalAndLtf; 4] = [
    HeGuardIntervalAndLtf::OneLtf800Ns,
    HeGuardIntervalAndLtf::TwoLtf800Ns,
    HeGuardIntervalAndLtf::TwoLtf1600Ns,
    HeGuardIntervalAndLtf::FourLtf3200Ns,
];

fn he_mcs() -> impl Iterator<Item = HeMcs> {
    (0..10).map(|index| HeMcs::from_index(index).unwrap())
}

fn he(rate: HeRate) -> PpduTiming {
    TxPhyRate::He(rate).ppdu_timing().unwrap()
}

#[test]
fn he_su_duration_follows_preamble_symbols_and_packet_extension() {
    // MCS0 1x/0.8: 12022 bits in 103 symbols, a = 3: 40 + 1400.8 + 12 + 6.
    let mcs0 = HeRate::new(HeMcs::Mcs0, HeGuardIntervalAndLtf::OneLtf800Ns);
    assert_eq!(he(mcs0).duration_micros(1500), 1459);
    // MCS7 2x/0.8: 11 symbols, 322-bit excess, a = 2: 43.2 + 149.6 + 8 + 6.
    let mcs7 = HeRate::new(HeMcs::Mcs7, HeGuardIntervalAndLtf::TwoLtf800Ns);
    assert_eq!(he(mcs7).duration_micros(1500), 207);
    // MCS9 4x/3.2: 8 symbols, 1102-bit excess, a = 3: 52 + 128 + 12 + 6.
    let mcs9 = HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::FourLtf3200Ns);
    assert_eq!(he(mcs9).duration_micros(1500), 198);
    // LDPC: 1096-bit excess, a_init = 3, N_punc 168 below 259.2: no extra.
    assert_eq!(
        he(HeRate::ldpc(
            HeMcs::Mcs9,
            HeGuardIntervalAndLtf::FourLtf3200Ns
        ))
        .duration_micros(1500),
        198
    );
    // The smallest S-MPDU, MCS0 2x/1.6: 118 bits in 2 symbols, a = 1:
    // 44 + 28.8 + 4 + 6.
    let short = HeRate::new(HeMcs::Mcs0, HeGuardIntervalAndLtf::TwoLtf1600Ns);
    assert_eq!(he(short).duration_micros(12), 83);
}

#[test]
fn he_su_ldpc_differs_from_bcc_where_its_extra_segment_lands() {
    // MCS7 1x/0.8, 140 bytes: BCC fits one symbol (a = 4); LDPC's extra
    // segment after a full last symbol adds a second one (a = 1).
    let gi = HeGuardIntervalAndLtf::OneLtf800Ns;
    assert_eq!(he(HeRate::new(HeMcs::Mcs7, gi)).duration_micros(140), 76);
    assert_eq!(he(HeRate::ldpc(HeMcs::Mcs7, gi)).duration_micros(140), 78);
    // MCS9 4x/3.2, 47 bytes: BCC a = 1, LDPC a = 2.
    let gi = HeGuardIntervalAndLtf::FourLtf3200Ns;
    assert_eq!(he(HeRate::new(HeMcs::Mcs9, gi)).duration_micros(47), 78);
    assert_eq!(he(HeRate::ldpc(HeMcs::Mcs9, gi)).duration_micros(47), 82);
}

#[test]
fn he_dcm_rates_stay_unknown() {
    use crate::mac::tx::{HeBccDcmMcs, HeLdpcDcmMcs};
    for gi in HE_GI_LTF {
        assert_eq!(
            TxPhyRate::He(HeRate::bcc_dcm(HeBccDcmMcs::Mcs0, gi)).ppdu_timing(),
            None
        );
        assert_eq!(
            TxPhyRate::He(HeRate::ldpc_dcm(HeLdpcDcmMcs::Mcs4, gi)).ppdu_timing(),
            None
        );
    }
}

#[test]
fn he_su_geometry_matches_code_rate_and_the_vendor_nominal_rate() {
    for mcs in he_mcs() {
        for gi in HE_GI_LTF {
            let ldpc = HeRate::ldpc(mcs, gi).su_timing().unwrap();
            let HeSuCoding::Ldpc(code_rate) = ldpc.coding else {
                panic!("an LDPC rate keeps its coding");
            };
            let (numerator, denominator) = match code_rate {
                LdpcCodeRate::Half => (1, 2),
                LdpcCodeRate::TwoThirds => (2, 3),
                LdpcCodeRate::ThreeQuarters => (3, 4),
                LdpcCodeRate::FiveSixths => (5, 6),
            };
            // N_DBPS = N_CBPS x R, for full and short symbols.
            assert_eq!(
                u32::from(ldpc.data_bits_per_symbol.get()) * denominator,
                u32::from(ldpc.coded_bits_per_symbol.get()) * numerator
            );
            assert_eq!(
                u32::from(ldpc.short_data_bits_per_symbol.get()) * denominator,
                u32::from(ldpc.short_coded_bits_per_symbol.get()) * numerator
            );
            let bcc = HeRate::new(mcs, gi).su_timing().unwrap();
            assert_eq!(bcc.coding, HeSuCoding::Bcc);
            assert_eq!(
                HeSuTiming {
                    coding: bcc.coding,
                    ..ldpc
                },
                bcc
            );
            // The vendor's RU242 table rounds N_DBPS / T_SYM to 100 kbit/s.
            let modelled = u32::from(bcc.data_bits_per_symbol.get()) * 1_000_000
                / u32::from(bcc.symbol_nanos.get());
            let vendor = HeRate::new(mcs, gi).nominal_kbps();
            assert!(modelled.abs_diff(vendor) <= 50, "{mcs:?} {gi:?}");
        }
    }
}

#[test]
fn every_he_su_rate_inverts_its_duration_at_each_budget() {
    for mcs in he_mcs() {
        for gi in HE_GI_LTF {
            for rate in [HeRate::new(mcs, gi), HeRate::ldpc(mcs, gi)] {
                let timing = he(rate);
                for micros in [0, 50, 63, 64, 100, 250, 1000, 4000, 5484, 10000] {
                    match timing.maximum_psdu_bytes(micros) {
                        Some(bytes) => {
                            assert!(timing.duration_micros(bytes.get()) <= micros);
                            if let Some(next) = bytes.get().checked_add(1) {
                                assert!(timing.duration_micros(next) > micros);
                            }
                        }
                        None => assert!(timing.duration_micros(1) > micros),
                    }
                }
            }
        }
    }
}
