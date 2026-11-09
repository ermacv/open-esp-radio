extern crate std;

use super::*;
use oer_protocols_ieee80211_wire::phy::{HeGiLtf, HtMcs, HtRate, PpduBandwidth};

fn ht(mcs: u8, width: PpduBandwidth, short_gi: bool) -> PhyRate {
    PhyRate::Ht(HtRate::new(HtMcs::new(mcs).unwrap(), width, short_gi).unwrap())
}

#[test]
fn an_unknown_cost_is_charged_the_backends_ceiling_of_the_executed_bounds() {
    let costs = S31Ceiling::new();
    assert_eq!(AGGREGATE_MEMBER_ATTEMPTS, 32);
    assert_eq!(MAX_SCHEDULE_PUBLICATION_LIMIT, 32);
    assert_eq!(
        costs.ceiling(1514),
        msdu_cost_ceiling_micros(1514, 32, 32).unwrap()
    );
    // A longer frame is never charged less, and a frame shorter than its
    // header is charged the most.
    assert!(costs.ceiling(1514) > costs.ceiling(64));
    assert_eq!(costs.ceiling(13), NonZeroU32::MAX);
}

/// The grant goes at the rate the station's air resolved: an OFDM-only
/// association's 6 Mb/s or a DSSS one's 1 Mb/s give different grants at one
/// length, each its own PPDU.
#[test]
fn the_grant_goes_at_the_resolved_rate_and_two_associations_floors_differ() {
    use oer_protocols_ieee80211_wire::phy::LegacyRate;
    let costs = S31Ceiling::new();
    let octets = 1514 + MPDU_GROWTH;
    let one = PhyRate::Legacy(LegacyRate::Dsss1M);
    let six = PhyRate::Legacy(LegacyRate::Ofdm6M);
    let at = |rate: PhyRate| rate.max_ppdu_duration_micros(octets).unwrap();
    assert_eq!(costs.least(1514, one).get(), at(one));
    assert_eq!(costs.least(1514, six).get(), at(six));
    assert_eq!(costs.least(1514, one).get(), at(one), "back again");
    assert_ne!(at(one), at(six));
    let fast = ht(7, PpduBandwidth::Mhz20, true);
    assert!(costs.least(1514, fast) < costs.least(1514, six));
    assert!(costs.least(1514, one) < costs.ceiling(1514));
    assert!(costs.least(1514, fast) > costs.least(64, fast));
}

/// Every input of the least grant is in its key: the same length at
/// another MCS, width, guard interval or PHY mode computes again, and each
/// answer is the one computed afresh, in any order.
#[test]
fn the_same_length_at_another_rate_computes_its_grant_again() {
    let costs = S31Ceiling::new();
    let he = |gi_ltf| {
        use oer_protocols_ieee80211_wire::phy::{FecCoding, HeMcs, HeRate, SpatialStreams};
        PhyRate::He(
            HeRate::new(
                HeMcs::new(9).unwrap(),
                SpatialStreams::ONE,
                PpduBandwidth::Mhz20,
                gi_ltf,
                FecCoding::Bcc,
                false,
            )
            .unwrap(),
        )
    };
    let rates = [
        ht(7, PpduBandwidth::Mhz20, true),
        ht(7, PpduBandwidth::Mhz20, false),
        ht(7, PpduBandwidth::Mhz40, true),
        ht(0, PpduBandwidth::Mhz20, true),
        he(HeGiLtf::Ltf1xGi800Ns),
        he(HeGiLtf::Ltf4xGi3200Ns),
    ];
    for _ in 0..2 {
        for rate in rates {
            for length in [1514, 1514, 64] {
                assert_eq!(
                    costs.least(length, rate),
                    least_at(length, rate),
                    "{rate:?} {length}"
                );
            }
        }
    }
    let distinct: std::collections::BTreeSet<u32> = rates
        .iter()
        .map(|rate| least_at(1514, *rate).get())
        .collect();
    assert_eq!(distinct.len(), rates.len(), "each rate its own grant");
}

#[test]
fn the_ceiling_is_computed_for_each_length() {
    let costs = S31Ceiling::new();
    for length in [1514, 64, 1514, 300] {
        assert_eq!(costs.ceiling(length), ceiling_of(length));
    }
}

/// No frame of a live association is granted a near-zero time: a frame no
/// PPDU at its rate carries asks the most, never the least.
#[test]
fn a_frame_no_ppdu_carries_asks_the_most_never_a_near_zero_grant() {
    use oer_protocols_ieee80211_wire::phy::LegacyRate;
    let costs = S31Ceiling::new();
    let one = PhyRate::Legacy(LegacyRate::Dsss1M);
    assert_eq!(costs.least(usize::MAX, one), NonZeroU32::MAX);
    for length in [14, 64, 1514, 2304] {
        assert!(costs.least(length, one).get() > 1, "{length}");
    }
}
