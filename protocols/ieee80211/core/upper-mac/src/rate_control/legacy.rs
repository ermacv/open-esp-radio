//! The nonempty legacy-rate set carried from association into rate selection.

use oer_ieee80211_wire::phy::LegacyRate;
use oer_ieee80211_wire::rates::{LEGACY_RATES, LegacyRateSet, RateSelection};

/// All legacy rates admitted by an association. Unlike
/// [`BasicRates`](crate::protection::BasicRates), this
/// includes rates without the basic bit. Construction requires at least one
/// recognized rate; an empty advertisement cannot imply OFDM support.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NegotiatedLegacyRates {
    set: LegacyRateSet,
    /// Always a member, so selection never needs an out-of-set fallback.
    first: LegacyRate,
    /// [`Self::constrain`] of each entry of [`LEGACY_RATES`], by its index;
    /// `None` where the rate is admitted and keeps its preamble. Rate plans
    /// constrain every retry stage, so this is read, not computed.
    constrained: [Option<LegacyRate>; LEGACY_RATES.len()],
}

impl NegotiatedLegacyRates {
    /// Read Supported Rates and Extended Supported Rates element bodies.
    /// The caller supplies the association's common rates. Entries naming
    /// membership selectors or unsupported encodings do not name legacy rates.
    pub fn from_rate_elements(supported: &[u8], extended: &[u8]) -> Option<Self> {
        let set = LegacyRateSet::from_rate_elements(supported, extended, RateSelection::All);
        Self::of_set(set)
    }

    /// The rates of a nonempty `set`.
    fn of_set(set: LegacyRateSet) -> Option<Self> {
        let first = set.rates().next()?;
        let mut constrained = [None; LEGACY_RATES.len()];
        for (slot, (_, rate)) in constrained.iter_mut().zip(LEGACY_RATES) {
            let chosen = constrain_in(set, first, rate);
            *slot = (chosen.kbps() != rate.kbps()).then_some(chosen);
        }
        Some(Self {
            set,
            first,
            constrained,
        })
    }

    /// Every admitted rate, with the long preamble for DSSS/CCK.
    pub fn rates(self) -> impl Iterator<Item = LegacyRate> {
        self.set.rates()
    }

    /// Support is independent of the separately negotiated DSSS preamble.
    pub fn contains(self, rate: LegacyRate) -> bool {
        self.rates()
            .any(|candidate| candidate.kbps() == rate.kbps())
    }

    /// Whether a legacy association can use an ERP-OFDM schedule.
    pub fn has_ofdm(self) -> bool {
        self.rates().any(LegacyRate::is_ofdm)
    }

    /// The fastest admitted rate no faster than `proposed`, or the slowest
    /// admitted rate if all exceed it. An already admitted rate keeps its
    /// selected preamble; another DSSS/CCK rate uses the long preamble.
    pub fn constrain(self, proposed: LegacyRate) -> LegacyRate {
        let kbps = proposed.kbps();
        LEGACY_RATES
            .iter()
            .position(|(_, rate)| rate.kbps() == kbps)
            .and_then(|index| self.constrained.get(index).copied())
            .map_or_else(
                || constrain_in(self.set, self.first, proposed),
                |chosen| chosen.unwrap_or(proposed),
            )
    }
}

/// The fastest rate of `set` no faster than `proposed`, or its slowest if
/// all exceed it; `proposed` itself when admitted. One pass, each the first
/// of equals.
fn constrain_in(set: LegacyRateSet, first: LegacyRate, proposed: LegacyRate) -> LegacyRate {
    let wanted = proposed.kbps();
    let mut slowest = first;
    let mut fastest_below: Option<LegacyRate> = None;
    for rate in set.rates() {
        let kbps = rate.kbps();
        if kbps == wanted {
            return proposed;
        }
        if kbps < slowest.kbps() {
            slowest = rate;
        }
        if kbps < wanted && fastest_below.is_none_or(|best| kbps > best.kbps()) {
            fastest_below = Some(rate);
        }
    }
    fastest_below.unwrap_or(slowest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_ieee80211_wire::phy::DsssPreamble;

    #[test]
    fn selection_keeps_supported_nonbasic_and_extended_rates() {
        let rates =
            NegotiatedLegacyRates::from_rate_elements(&[0x82, 0x84, 0x96], &[0x0c, 0x30]).unwrap();
        assert_eq!(rates.rates().count(), 5);
        assert!(rates.contains(LegacyRate::Ofdm6M));
        assert!(rates.contains(LegacyRate::Ofdm24M));
        assert_eq!(rates.constrain(LegacyRate::Ofdm54M), LegacyRate::Ofdm24M);
        assert_eq!(rates.constrain(LegacyRate::Ofdm9M), LegacyRate::Ofdm6M);
        assert_eq!(
            rates.constrain(LegacyRate::Cck11M(DsssPreamble::Short)),
            LegacyRate::Cck11M(DsssPreamble::Short)
        );
        let single = NegotiatedLegacyRates::from_rate_elements(&[0x98], &[]).unwrap();
        assert_eq!(single.constrain(LegacyRate::Dsss1M), LegacyRate::Ofdm12M);
        assert!(NegotiatedLegacyRates::from_rate_elements(&[], &[]).is_none());
        assert!(NegotiatedLegacyRates::from_rate_elements(&[0xff, 0], &[0xfe]).is_none());
    }

    /// The former three-pass definition, which the single pass keeps.
    fn constrained(rates: NegotiatedLegacyRates, proposed: LegacyRate) -> LegacyRate {
        if rates.contains(proposed) {
            return proposed;
        }
        let slowest = rates
            .rates()
            .fold(rates.first, |a, b| if b.kbps() < a.kbps() { b } else { a });
        rates
            .rates()
            .filter(|rate| rate.kbps() <= proposed.kbps())
            .fold(slowest, |a, b| if b.kbps() > a.kbps() { b } else { a })
    }

    #[test]
    fn the_single_pass_selects_as_the_definition_for_every_set_and_rate() {
        use oer_ieee80211_wire::rates::LegacyRateSet;
        let proposals = LEGACY_RATES.iter().map(|(_, rate)| *rate).chain([
            LegacyRate::Dsss2M(DsssPreamble::Short),
            LegacyRate::Cck5M5(DsssPreamble::Short),
            LegacyRate::Cck11M(DsssPreamble::Short),
        ]);
        let proposals: std::vec::Vec<LegacyRate> = proposals.collect();
        for bits in 1_u16..(1 << LEGACY_RATES.len()) {
            let set = LegacyRateSet::from_bits(bits);
            let rates = NegotiatedLegacyRates::of_set(set).unwrap();
            for &proposed in &proposals {
                assert_eq!(
                    rates.constrain(proposed),
                    constrained(rates, proposed),
                    "{bits:#x} {proposed:?}"
                );
            }
        }
    }
}
