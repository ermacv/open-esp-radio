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

#[test]
fn before_a_report_the_least_grant_goes_at_the_slowest_single_rate() {
    let costs = S31Ceiling::new();
    let octets = 1514 + MPDU_GROWTH;
    assert_eq!(
        costs.least(1514, None).get(),
        SLOWEST_SINGLE.max_ppdu_duration_micros(octets).unwrap()
    );
    let fast = ht(7, PpduBandwidth::Mhz20, true);
    assert!(costs.least(1514, Some(fast)) < costs.least(1514, None));
    assert!(costs.least(1514, None) < costs.ceiling(1514));
    assert!(costs.least(1514, Some(fast)) > costs.least(64, Some(fast)));
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
        Some(ht(7, PpduBandwidth::Mhz20, true)),
        Some(ht(7, PpduBandwidth::Mhz20, false)),
        Some(ht(7, PpduBandwidth::Mhz40, true)),
        Some(ht(0, PpduBandwidth::Mhz20, true)),
        Some(he(HeGiLtf::Ltf1xGi800Ns)),
        Some(he(HeGiLtf::Ltf4xGi3200Ns)),
        None,
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
