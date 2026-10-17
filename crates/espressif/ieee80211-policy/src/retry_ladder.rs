//! The Espressif ordinary-MPDU retry ladder: the rate of each attempt of an
//! MPDU after failures, from the vendor rate schedules.
//!
//! SOURCE(esp32s31): `libpp[trc.o]::rcGetRate, libpp[trc.o]::rcUpdatePhyMode`. `libpp[trc.o]::rcGetRate`
//! walks the four `(rate, count)` pairs of the schedule record the frame's
//! first rate selects ([`crate::rate_schedule::schedule_rate_after_failures`]);
//! the record comes from `rcUpdatePhyMode`'s mapping of that rate. The
//! ladder is the vendor's policy and is inseparable from its schedule bytes,
//! so it lives here beside them; the portable retry state machine
//! (`oer-ieee80211-upper-mac::retry`) consumes it through
//! [`RateLadder`].

use oer_ieee80211_mac::phy::{
    FecCoding, HeGiLtf, HeRate, HtRate, LegacyRate, PhyRate, PpduBandwidth,
};
use oer_ieee80211_upper_mac::RateLadder;

use crate::{
    rate_code::{legacy_code, legacy_rate, phy_rate},
    rate_schedule::{
        RateScheduleKind, RateScheduleRef, dot11g_schedule_for_legacy_rate,
        schedule_publication_limit, schedule_rate_after_failures,
    },
};

/// The 802.11g ladder of a non-HT rate: the complete 54 Mb/s ladder is
/// `54M x2, 48M x2, 6M x3, 5.5M x25`; the other rates use their
/// corresponding records. `None` once the record's complete retry budget is
/// consumed.
pub fn legacy_retry_rate(initial: LegacyRate, failed_attempts: u8) -> Option<LegacyRate> {
    let schedule = dot11g_schedule_for_legacy_rate(legacy_code(initial))?;
    legacy_rate(schedule_rate_after_failures(schedule, failed_attempts)?)
}

/// Hardware publications the non-HT rate's record admits, the first
/// included.
///
/// This is not the first retry pair's count: the vendor
/// `rcReachRetryLimit` reads record byte `0x08`, while `rcGetRate` consumes
/// the four `(rate, count)` pairs independently.
pub fn legacy_retry_publication_limit(initial: LegacyRate) -> Option<u8> {
    let schedule = dot11g_schedule_for_legacy_rate(legacy_code(initial))?;
    Some(schedule_publication_limit(schedule))
}

/// The 802.11n ladder of an HT rate; the rate may leave the HT domain.
///
/// The records cover every one-stream long-GI MCS 0-7 and short-GI MCS 7,
/// the vendor's maximum-throughput starting point. Other short-GI MCSs have
/// no record in the complete `rcUpdatePhyMode` mapping and return `None`
/// instead of an invented fallback, as do rates of more than one stream.
pub fn ht_retry_rate(initial: HtRate, failed_attempts: u8) -> Option<PhyRate> {
    let mcs = initial.mcs().index();
    if mcs > 7 {
        return None;
    }
    let index = match initial.short_gi() {
        // mcs <= 7 above, so this is the exact record index 1..=8.
        false => 8_u8.wrapping_sub(mcs),
        true if mcs == 7 => 0,
        true => return None,
    };
    let schedule = RateScheduleRef::new(RateScheduleKind::Dot11N, index)?;
    let code = schedule_rate_after_failures(schedule, failed_attempts)?;
    phy_rate(
        RateScheduleKind::Dot11N,
        code,
        initial.bandwidth(),
        HeGiLtf::Ltf2xGi800Ns,
    )
}

/// The 802.11ax ladder of an HE20 SU one-stream rate; the rate may leave
/// the HE domain.
///
/// The records hold one dedicated 0.8 µs MCS 9 entry and ten 1.6 µs entries
/// for MCS 9 through 0. FEC is not part of the schedule byte: a retry that
/// stays HE keeps the initial rate's BCC or LDPC. DCM has its own producer
/// and returns `None`, as do other guard intervals, widths and stream
/// counts.
pub fn he_retry_rate(initial: HeRate, failed_attempts: u8) -> Option<PhyRate> {
    if initial.dcm()
        || initial.bandwidth() != PpduBandwidth::Mhz20
        || initial.spatial_streams().count() != 1
        || initial.mcs().index() > 9
    {
        return None;
    }
    let mcs = initial.mcs().index();
    let index = match initial.gi_ltf() {
        HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns if mcs == 9 => 0,
        // The admission above proves mcs <= 9, hence index 1..=10.
        HeGiLtf::Ltf2xGi1600Ns => 10_u8.wrapping_sub(mcs),
        HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns | HeGiLtf::Ltf4xGi3200Ns => return None,
    };
    let schedule = RateScheduleRef::new(RateScheduleKind::Dot11Ax, index)?;
    let code = schedule_rate_after_failures(schedule, failed_attempts)?;
    let selected = phy_rate(
        RateScheduleKind::Dot11Ax,
        code,
        PpduBandwidth::Mhz20,
        initial.gi_ltf(),
    )?;
    Some(match (selected, initial.fec()) {
        (PhyRate::He(rate), FecCoding::Ldpc) => PhyRate::He(HeRate::new(
            rate.mcs(),
            rate.spatial_streams(),
            rate.bandwidth(),
            rate.gi_ltf(),
            FecCoding::Ldpc,
            false,
        )?),
        (selected, _) => selected,
    })
}

/// The ordinary retry ladder: the rate of the attempt after
/// `failed_attempts` failures of a frame first sent at `initial`.
///
/// `None` when the rate has no record or the record's budget is consumed;
/// the portable retry state then keeps the previous rate.
pub fn ordinary_retry_rate(initial: PhyRate, failed_attempts: u8) -> Option<PhyRate> {
    match initial {
        PhyRate::Legacy(rate) => legacy_retry_rate(rate, failed_attempts).map(PhyRate::Legacy),
        PhyRate::Ht(rate) => ht_retry_rate(rate, failed_attempts),
        PhyRate::He(rate) => he_retry_rate(rate, failed_attempts),
    }
}

/// [`ordinary_retry_rate`] as the portable retry state's ladder.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EspressifRetryLadder;

impl RateLadder for EspressifRetryLadder {
    fn rate(&self, initial: PhyRate, failures: u8) -> Option<PhyRate> {
        ordinary_retry_rate(initial, failures)
    }
}

#[cfg(test)]
mod tests {
    use oer_ieee80211_mac::phy::{DsssPreamble, HeMcs, HtMcs, SpatialStreams};

    use super::*;

    #[test]
    fn the_54_megabit_ladder_steps_down_to_cck() {
        let rate = |failures| legacy_retry_rate(LegacyRate::Ofdm54M, failures);
        assert_eq!(rate(0), Some(LegacyRate::Ofdm54M));
        assert_eq!(rate(2), Some(LegacyRate::Ofdm48M));
        assert_eq!(rate(4), Some(LegacyRate::Ofdm6M));
        assert_eq!(rate(7), Some(LegacyRate::Cck5M5(DsssPreamble::Short)));
        assert_eq!(rate(32), None);
        assert_eq!(
            legacy_retry_publication_limit(LegacyRate::Ofdm54M),
            Some(32)
        );
    }

    #[test]
    fn ht_and_he_ladders_keep_their_width_and_coding() {
        let ht40 = HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz40, false).unwrap();
        assert_eq!(ht_retry_rate(ht40, 0), Some(PhyRate::Ht(ht40)));
        assert!(matches!(ht_retry_rate(ht40, 2), Some(PhyRate::Ht(rate))
            if rate.bandwidth() == PpduBandwidth::Mhz40 && rate.mcs().index() == 5));
        let sgi_mcs3 = HtRate::new(HtMcs::new(3).unwrap(), PpduBandwidth::Mhz20, true).unwrap();
        assert_eq!(ht_retry_rate(sgi_mcs3, 0), None);

        let he = HeRate::new(
            HeMcs::new(9).unwrap(),
            SpatialStreams::ONE,
            PpduBandwidth::Mhz20,
            HeGiLtf::Ltf2xGi1600Ns,
            FecCoding::Ldpc,
            false,
        )
        .unwrap();
        assert!(matches!(he_retry_rate(he, 2), Some(PhyRate::He(rate))
            if rate.mcs().index() == 7 && rate.fec() == FecCoding::Ldpc));
        assert_eq!(
            EspressifRetryLadder.rate(PhyRate::He(he), 0),
            Some(PhyRate::He(he))
        );
    }

    #[test]
    fn every_supported_starting_mcs_uses_its_own_record() {
        for index in 0..=31 {
            for short in [false, true] {
                let initial =
                    HtRate::new(HtMcs::new(index).unwrap(), PpduBandwidth::Mhz20, short).unwrap();
                let expected = if index <= 7 && (!short || index == 7) {
                    Some(PhyRate::Ht(initial))
                } else {
                    None
                };
                assert_eq!(ht_retry_rate(initial, 0), expected);
            }
        }
        for index in 0..=11 {
            for gi in [
                HeGiLtf::Ltf1xGi800Ns,
                HeGiLtf::Ltf2xGi800Ns,
                HeGiLtf::Ltf2xGi1600Ns,
                HeGiLtf::Ltf4xGi3200Ns,
            ] {
                let initial = HeRate::new(
                    HeMcs::new(index).unwrap(),
                    SpatialStreams::ONE,
                    PpduBandwidth::Mhz20,
                    gi,
                    FecCoding::Ldpc,
                    false,
                )
                .unwrap();
                let expected = if (index <= 9 && gi == HeGiLtf::Ltf2xGi1600Ns)
                    || (index == 9 && matches!(gi, HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns))
                {
                    Some(PhyRate::He(initial))
                } else {
                    None
                };
                assert_eq!(he_retry_rate(initial, 0), expected);
            }
        }
    }
}
