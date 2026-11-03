//! Data A-MPDU limits and the retry policy under the station's transmit
//! Block Ack agreements.
//!
//! [`PpduLimit`] bounds one A-MPDU in subframes and octets at its rate, so
//! a frame's admission to an arena is one addition and one comparison.
//! [`AggregateRetry`] is how long a frame a BlockAck missed is retried.

use super::DataProtection;

/// The most subframes one data aggregate holds: the width of the port's
/// subframe masks.
pub const MAX_AGGREGATE: usize =
    oer_protocols_ieee80211_contracts_port::arena::AMPDU_SUBFRAME_MASK_BITS;

/// The most one aggregate takes: subframes, and octets with delimiters and
/// padding. The octet bound holds the length limits and the rate's PPDU
/// time limit at once, computed when the rate or the agreement changes, so
/// a member's admission is one addition and one comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PpduLimit {
    subframes: usize,
    octets: u32,
}

impl PpduLimit {
    /// At most `subframes` subframes (and [`MAX_AGGREGATE`]) and
    /// `max_length` octets (the recipient's Maximum A-MPDU Length and the
    /// port's at the aggregate's rate, the lower), within `rate`'s PPDU time
    /// limit ([`PhyRate::max_txtime_micros`](oer_protocols_ieee80211_contracts_port::PhyRate::max_txtime_micros));
    /// `None` checks no time. The time bound counts the whole aggregate at
    /// the rate (IEEE Std 802.11-2020 19.4.3, through
    /// `PhyRate::max_ppdu_duration_micros`), which grows with the octets:
    /// the bound is the longest aggregate within it.
    pub fn new(
        subframes: usize,
        max_length: u32,
        rate: Option<oer_protocols_ieee80211_contracts_port::PhyRate>,
    ) -> Self {
        let timed = rate.and_then(|rate| rate.max_txtime_micros().map(|limit| (rate, limit)));
        let octets = match timed {
            None => max_length,
            Some((rate, limit)) => longest_within(max_length, |octets| {
                rate.max_ppdu_duration_micros(octets)
                    .is_some_and(|duration| duration <= limit)
            }),
        };
        Self {
            subframes: subframes.min(MAX_AGGREGATE),
            octets,
        }
    }

    /// The most subframes one aggregate takes, at most [`MAX_AGGREGATE`].
    pub const fn subframes(&self) -> usize {
        self.subframes
    }

    /// The longest aggregate in octets, delimiters and padding included.
    pub const fn octets(&self) -> u32 {
        self.octets
    }

    /// Whether an aggregate of `octets`, a positive count, stays within the
    /// limit.
    #[cfg(test)]
    const fn fits(&self, octets: u32) -> bool {
        octets <= self.octets
    }
}

/// The largest count up to `max` that `fits`, which holds up to a count and
/// fails above it; zero when it holds for none above zero.
fn longest_within(max: u32, fits: impl Fn(u32) -> bool) -> u32 {
    if fits(max) {
        return max;
    }
    // `low` fits or is zero; `high` does not fit.
    let (mut low, mut high) = (0_u32, max);
    while high.saturating_sub(low) > 1 {
        let middle = low.saturating_add(high.saturating_sub(low) / 2);
        if fits(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

/// The octets one member takes in an aggregate: its delimiter, its MPDU
/// with the CCMP MIC the port appends to a protected one and the FCS,
/// padded to a four-octet boundary.
///
/// SOURCE: IEEE Std 802.11-2020, 9.7.1 (A-MPDU format: four-octet delimiter
/// and padding to a multiple of four octets).
pub(super) fn subframe_octets(mpdu: usize, protection: DataProtection<()>) -> Option<u32> {
    const DELIMITER: usize = 4;
    const FCS: usize = 4;
    let mic = match protection {
        DataProtection::Open => 0,
        DataProtection::Ccmp(()) => oer_protocols_ieee80211_wire::ccmp::CCMP_MIC_LEN,
    };
    let unpadded = mpdu.checked_add(mic)?.checked_add(FCS)?;
    let padded = unpadded.checked_add(3)? & !3;
    u32::try_from(padded.checked_add(DELIMITER)?).ok()
}

/// How long a missing member is retried: the composition's policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AggregateRetry {
    /// A member whose MSDU is this old is dropped instead of retried.
    pub lifetime: oer_portable_time::Duration,
    /// Aggregates on the air a member may miss before it is dropped.
    pub attempts: u8,
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_protocols_ieee80211_contracts_port::PhyRate;
    use oer_protocols_ieee80211_wire::phy::{HtMcs, HtRate, LegacyRate, PpduBandwidth};

    /// The admission the octet bound replaces: the length, then the rate's
    /// time limit over the whole aggregate at the rate.
    fn fits_in_time(max_length: u32, rate: Option<PhyRate>, octets: u32) -> bool {
        octets <= max_length
            && rate.is_none_or(|rate| {
                rate.max_txtime_micros().is_none_or(|limit| {
                    rate.max_ppdu_duration_micros(octets)
                        .is_some_and(|duration| duration <= limit)
                })
            })
    }

    fn ht_rates() -> impl Iterator<Item = PhyRate> {
        (0..=31).flat_map(|mcs| {
            [PpduBandwidth::Mhz20, PpduBandwidth::Mhz40]
                .into_iter()
                .flat_map(move |bandwidth| {
                    [false, true].map(move |short_gi| {
                        PhyRate::Ht(
                            HtRate::new(HtMcs::new(mcs).unwrap(), bandwidth, short_gi).unwrap(),
                        )
                    })
                })
        })
    }

    #[test]
    fn the_octet_bound_admits_every_ht_aggregate_the_time_limit_admits_and_no_other() {
        for rate in ht_rates() {
            let limit = PpduLimit::new(MAX_AGGREGATE, 65_535, Some(rate));
            for octets in 1..=70_000 {
                assert_eq!(
                    limit.fits(octets),
                    fits_in_time(65_535, Some(rate), octets),
                    "{rate:?} at {octets} octets"
                );
            }
        }
    }

    #[test]
    fn the_octet_bound_keeps_every_length_limit_at_its_boundaries() {
        let rates = ht_rates()
            .map(Some)
            .chain([
                Some(PhyRate::Legacy(LegacyRate::Dsss1M)),
                Some(PhyRate::Legacy(LegacyRate::Ofdm6M)),
                Some(PhyRate::Legacy(LegacyRate::Ofdm54M)),
                None,
            ])
            .collect::<std::vec::Vec<_>>();
        for max_length in [
            1,
            4_095,
            8_191,
            16_383,
            32_767,
            65_535,
            262_143,
            1_048_575,
            u32::MAX,
        ] {
            for &rate in &rates {
                let limit = PpduLimit::new(MAX_AGGREGATE, max_length, rate);
                let bound = limit.octets();
                for octets in [
                    1,
                    bound.saturating_sub(1),
                    bound,
                    bound.saturating_add(1),
                    max_length,
                    max_length.saturating_add(1),
                    u32::MAX,
                ] {
                    assert_eq!(
                        limit.fits(octets),
                        fits_in_time(max_length, rate, octets),
                        "{rate:?} within {max_length} at {octets} octets"
                    );
                }
            }
        }
    }

    #[test]
    fn the_subframe_bound_is_at_most_the_ports_mask_width() {
        assert_eq!(PpduLimit::new(3, 65_535, None).subframes(), 3);
        assert_eq!(
            PpduLimit::new(MAX_AGGREGATE + 1, 65_535, None).subframes(),
            MAX_AGGREGATE
        );
    }
}
