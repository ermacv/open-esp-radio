//! The Espressif `wifi_phy_rate_t` rate codes of the rate schedules, as
//! portable rates.
//!
//! SOURCE: Espressif's `esp_wifi_types_generic.h` (`wifi_phy_rate_t`), as
//! generated for the ESP32-S31 (`esp-wifi-sys-esp32s31`), cross-checked
//! against the `libpp.a` rate schedules. The numeric MCS domains overlap:
//! `0x10..=0x19` is HT long GI in an 802.11n schedule but HE MCS 0-9 with a
//! 1.6 µs guard interval in the 802.11ax schedule, and `0x1a..=0x23` is HT
//! short GI or HE MCS 0-9 with 0.8 µs. Decoding therefore names the schedule
//! the code came from. Codes `0x29` and `0x2a` are the vendor's Long Range
//! modes, which have no portable rate.

use oer_ieee80211_mac::phy::{
    DsssPreamble, FecCoding, HeGiLtf, HeMcs, HeRate, HtMcs, HtRate, LegacyRate, PhyRate,
    PpduBandwidth, SpatialStreams,
};

use crate::rate_schedule::RateScheduleKind;

/// The non-HT rate of a code.
pub const fn legacy_rate(code: u8) -> Option<LegacyRate> {
    Some(match code {
        0x00 => LegacyRate::Dsss1M,
        0x01 => LegacyRate::Dsss2M(DsssPreamble::Long),
        0x02 => LegacyRate::Cck5M5(DsssPreamble::Long),
        0x03 => LegacyRate::Cck11M(DsssPreamble::Long),
        0x05 => LegacyRate::Dsss2M(DsssPreamble::Short),
        0x06 => LegacyRate::Cck5M5(DsssPreamble::Short),
        0x07 => LegacyRate::Cck11M(DsssPreamble::Short),
        0x08 => LegacyRate::Ofdm48M,
        0x09 => LegacyRate::Ofdm24M,
        0x0a => LegacyRate::Ofdm12M,
        0x0b => LegacyRate::Ofdm6M,
        0x0c => LegacyRate::Ofdm54M,
        0x0d => LegacyRate::Ofdm36M,
        0x0e => LegacyRate::Ofdm18M,
        0x0f => LegacyRate::Ofdm9M,
        _ => return None,
    })
}

/// The code of a non-HT rate.
pub const fn legacy_code(rate: LegacyRate) -> u8 {
    match rate {
        LegacyRate::Dsss1M => 0x00,
        LegacyRate::Dsss2M(DsssPreamble::Long) => 0x01,
        LegacyRate::Cck5M5(DsssPreamble::Long) => 0x02,
        LegacyRate::Cck11M(DsssPreamble::Long) => 0x03,
        LegacyRate::Dsss2M(DsssPreamble::Short) => 0x05,
        LegacyRate::Cck5M5(DsssPreamble::Short) => 0x06,
        LegacyRate::Cck11M(DsssPreamble::Short) => 0x07,
        LegacyRate::Ofdm48M => 0x08,
        LegacyRate::Ofdm24M => 0x09,
        LegacyRate::Ofdm12M => 0x0a,
        LegacyRate::Ofdm6M => 0x0b,
        LegacyRate::Ofdm54M => 0x0c,
        LegacyRate::Ofdm36M => 0x0d,
        LegacyRate::Ofdm18M => 0x0e,
        LegacyRate::Ofdm9M => 0x0f,
    }
}

/// The portable rate of a code of `kind`'s schedule.
///
/// Legacy codes decode in every schedule. HT codes (`0x10..=0x17` long GI,
/// `0x1a..=0x21` short GI) are one-stream MCS 0-7 at `ht_bandwidth`. In the
/// 802.11ax schedule `0x10..=0x19` is HE20 SU MCS 0-9 with two HE-LTFs and
/// a 1.6 µs guard interval, and `0x1a..=0x23` the same MCS with
/// `he_800ns_gi_ltf`, which must be one of the 0.8 µs forms; HE rates are
/// BCC without DCM, which the code does not carry.
pub const fn phy_rate(
    kind: RateScheduleKind,
    code: u8,
    ht_bandwidth: PpduBandwidth,
    he_800ns_gi_ltf: HeGiLtf,
) -> Option<PhyRate> {
    if let Some(rate) = legacy_rate(code) {
        return Some(PhyRate::Legacy(rate));
    }
    if matches!(kind, RateScheduleKind::Dot11Ax) {
        // Each guarded range starts at the subtracted base and has ten
        // values. Both differences are exactly 0..=9, without wrap or clamp.
        let (index, gi_ltf) = if code >= 0x10 && code <= 0x19 {
            (code.wrapping_sub(0x10), HeGiLtf::Ltf2xGi1600Ns)
        } else if code >= 0x1a && code <= 0x23 {
            if !matches!(
                he_800ns_gi_ltf,
                HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns
            ) {
                return None;
            }
            (code.wrapping_sub(0x1a), he_800ns_gi_ltf)
        } else {
            return None;
        };
        let Some(mcs) = HeMcs::new(index) else {
            return None;
        };
        return match HeRate::new(
            mcs,
            SpatialStreams::ONE,
            PpduBandwidth::Mhz20,
            gi_ltf,
            FecCoding::Bcc,
            false,
        ) {
            Some(rate) => Some(PhyRate::He(rate)),
            None => None,
        };
    }
    // The HT ranges similarly prove an exact difference of 0..=7.
    let (index, short_gi) = if code >= 0x10 && code <= 0x17 {
        (code.wrapping_sub(0x10), false)
    } else if code >= 0x1a && code <= 0x21 {
        (code.wrapping_sub(0x1a), true)
    } else {
        return None;
    };
    let Some(mcs) = HtMcs::new(index) else {
        return None;
    };
    match HtRate::new(mcs, ht_bandwidth, short_gi) {
        Some(rate) => Some(PhyRate::Ht(rate)),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_codes_round_trip() {
        for code in 0..=0x0f {
            if let Some(rate) = legacy_rate(code) {
                assert_eq!(legacy_code(rate), code);
            }
        }
        assert_eq!(legacy_rate(0x04), None);
    }

    #[test]
    fn overlapping_mcs_codes_decode_by_their_schedule() {
        let ht = phy_rate(
            RateScheduleKind::Dot11N,
            0x17,
            PpduBandwidth::Mhz40,
            HeGiLtf::Ltf2xGi800Ns,
        );
        assert_eq!(
            ht,
            Some(PhyRate::Ht(
                HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz40, false).unwrap()
            ))
        );
        let he = phy_rate(
            RateScheduleKind::Dot11Ax,
            0x19,
            PpduBandwidth::Mhz20,
            HeGiLtf::Ltf2xGi800Ns,
        );
        assert!(matches!(he, Some(PhyRate::He(rate))
            if rate.mcs().index() == 9 && rate.gi_ltf() == HeGiLtf::Ltf2xGi1600Ns));
        // The 0.8 µs domain needs a 0.8 µs form.
        assert_eq!(
            phy_rate(
                RateScheduleKind::Dot11Ax,
                0x23,
                PpduBandwidth::Mhz20,
                HeGiLtf::Ltf4xGi3200Ns
            ),
            None
        );
        // Long Range codes have no portable rate.
        assert_eq!(
            phy_rate(
                RateScheduleKind::Dot11G,
                0x29,
                PpduBandwidth::Mhz20,
                HeGiLtf::Ltf2xGi800Ns
            ),
            None
        );
    }

    #[test]
    fn every_mcs_code_preserves_its_guard_interval_and_exact_index() {
        for kind in [RateScheduleKind::Dot11N, RateScheduleKind::Dot11Ax] {
            for gi in [
                HeGiLtf::Ltf1xGi800Ns,
                HeGiLtf::Ltf2xGi800Ns,
                HeGiLtf::Ltf2xGi1600Ns,
                HeGiLtf::Ltf4xGi3200Ns,
            ] {
                for code in 0x10..=u8::MAX {
                    let rate = phy_rate(kind, code, PpduBandwidth::Mhz40, gi);
                    // Signed, wider arithmetic independently detects either
                    // an underflowed base subtraction or a clamped index.
                    let long_index = i16::from(code) - 0x10;
                    let short_index = i16::from(code) - 0x1a;
                    match kind {
                        RateScheduleKind::Dot11N => {
                            let (index, short) = if (0..8).contains(&long_index) {
                                (long_index, false)
                            } else if (0..8).contains(&short_index) {
                                (short_index, true)
                            } else {
                                assert_eq!(rate, None);
                                continue;
                            };
                            let Some(PhyRate::Ht(rate)) = rate else {
                                panic!("HT code {code}")
                            };
                            assert_eq!(i16::from(rate.mcs().index()), index);
                            assert_eq!(rate.short_gi(), short);
                            assert_eq!(rate.bandwidth(), PpduBandwidth::Mhz40);
                        }
                        RateScheduleKind::Dot11Ax => {
                            let (index, gi) = if (0..10).contains(&long_index) {
                                (long_index, HeGiLtf::Ltf2xGi1600Ns)
                            } else if (0..10).contains(&short_index)
                                && matches!(gi, HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns)
                            {
                                (short_index, gi)
                            } else {
                                assert_eq!(rate, None);
                                continue;
                            };
                            let Some(PhyRate::He(rate)) = rate else {
                                panic!("HE code {code}")
                            };
                            assert_eq!(i16::from(rate.mcs().index()), index);
                            assert_eq!(rate.gi_ltf(), gi);
                            assert_eq!(rate.bandwidth(), PpduBandwidth::Mhz20);
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }
    }
}
