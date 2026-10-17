//! The nonempty legacy-rate set carried from association into rate selection.

use oer_ieee80211_mac::phy::LegacyRate;

use crate::BasicRates;

/// All legacy rates admitted by an association. Unlike `BasicRates`, this
/// includes rates without the basic bit. Construction requires at least one
/// recognized rate; an empty advertisement cannot imply OFDM support.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NegotiatedLegacyRates {
    mask: u16,
    /// Always a member, so selection never needs an out-of-set fallback.
    first: LegacyRate,
}

impl NegotiatedLegacyRates {
    /// Read Supported Rates and Extended Supported Rates element bodies.
    /// The caller supplies the association's common rates. Entries naming
    /// membership selectors or unsupported encodings do not name legacy rates.
    pub fn from_rate_elements(supported: &[u8], extended: &[u8]) -> Option<Self> {
        let mut mask = 0;
        let mut first = None;
        for (index, (units, rate)) in BasicRates::ORDER.into_iter().enumerate() {
            if supported
                .iter()
                .chain(extended)
                .any(|encoded| encoded & 0x7f == units)
            {
                mask |= 1 << index;
                first.get_or_insert(rate);
            }
        }
        Some(Self {
            mask,
            first: first?,
        })
    }

    /// Every admitted rate, with the long preamble for DSSS/CCK.
    pub fn rates(self) -> impl Iterator<Item = LegacyRate> {
        BasicRates::ORDER
            .into_iter()
            .enumerate()
            .filter(move |(index, _)| self.mask & (1 << index) != 0)
            .map(|(_, (_, rate))| rate)
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
        if self.contains(proposed) {
            return proposed;
        }
        let slowest = self
            .rates()
            .fold(self.first, |a, b| if b.kbps() < a.kbps() { b } else { a });
        self.rates()
            .filter(|rate| rate.kbps() <= proposed.kbps())
            .fold(slowest, |a, b| if b.kbps() > a.kbps() { b } else { a })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_ieee80211_mac::phy::DsssPreamble;

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
}
