//! The longest aggregate at each rate (#1278): an HT rate's is the
//! vendor's per-rate ceiling, an HE rate's the ROM's APEP at the default
//! TXOP, never the HT ceiling.

use super::*;
use crate::mac::rx::HeGuardIntervalAndLtf;
use crate::mac::tx::{HeMcs, HeRate, TxPhyRate};

#[test]
fn each_ht_rate_has_the_vendor_ceiling_as_its_length() {
    let ht = |mcs, guard| {
        esp32s31_ampdu_length(TxPhyRate::Ht(HtRate::new(
            mcs,
            guard,
            HtChannelWidth::Mhz20,
        )))
    };
    // MCS 0: the vendor's 6 490 octets at the long guard interval.
    assert_eq!(ht(HtMcs::Mcs0, HtGuardInterval::Long800Ns), Some(6_490));
    assert_eq!(ht(HtMcs::Mcs0, HtGuardInterval::Short400Ns), Some(9_600));
    // MCS 7: 65 535 octets at both.
    assert_eq!(ht(HtMcs::Mcs7, HtGuardInterval::Long800Ns), Some(65_535));
    // The vendor sets no ceiling at short-GI MCS 7: the HT Length field's.
    assert_eq!(
        ht(HtMcs::Mcs7, HtGuardInterval::Short400Ns),
        Some(ESP32S31_AMPDU_MAX_LENGTH)
    );
}

#[test]
fn an_he_rate_has_its_apep_as_its_length_not_the_ht_ceiling() {
    let he = |mcs| {
        esp32s31_ampdu_length(TxPhyRate::He(HeRate::new(
            mcs,
            HeGuardIntervalAndLtf::TwoLtf800Ns,
        )))
    };
    // The ROM's APEP at the default TXOP, above the HT ceiling's 6 490
    // octets but at HE-MCS 0.
    assert_eq!(
        [HeMcs::Mcs0, HeMcs::Mcs7, HeMcs::Mcs9].map(he),
        [Some(3_700), Some(37_600), Some(50_000)]
    );
}
