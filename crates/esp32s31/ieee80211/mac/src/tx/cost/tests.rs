use super::*;
use crate::tx::HtMcs;

fn block_ack() -> PpduTiming {
    TxPhyRate::Legacy(LegacyRate::Ofdm24M)
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
