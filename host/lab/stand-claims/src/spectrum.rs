//! The radio environment as frequency ranges.
//!
//! A lease that uses the radio claims the range it occupies with two
//! properties: what it needs from others there (`strict`: nobody transmits;
//! `tolerant`: nobody transmits noisily; `none`) and how it transmits
//! (`noisy`: a continuous carrier, transmission without CSMA, DTM or a
//! throughput flood; `normal`: protocol traffic; `none`: receive only). Two
//! claims conflict only where their ranges overlap: a strict need excludes any
//! transmission, a tolerant need excludes a noisy one.
//!
//! A range is the resource `spectrum:<low kHz>-<high kHz>:<need>:<emits>`,
//! claimed shared, so the state format is unchanged. Every such claim comes
//! with the coarse [`AIR`] claim, exclusive for a strict need or a noisy
//! transmission and shared otherwise. Arbiters without ranges order leases by
//! that claim alone, so a checkout that predates ranges still never overlaps
//! a strict measurement; two leases that both carry ranges are ordered by
//! their ranges instead.

use crate::{AIR, Claim, Mode};

const PREFIX: &str = "spectrum:";

/// What a lease needs from other transmitters in its range.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Need {
    None,
    Tolerant,
    Strict,
}

/// How a lease transmits in its range.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Emits {
    None,
    Normal,
    Noisy,
}

/// A frequency range, in kHz, with how a lease uses it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Spectrum {
    pub low_khz: u64,
    pub high_khz: u64,
    pub need: Need,
    pub emits: Emits,
}

/// The 2.4 GHz ISM band.
pub const BAND_2G4: (u64, u64) = (2_400_000, 2_483_500);
/// The 5 GHz Wi-Fi bands.
pub const BAND_5G: (u64, u64) = (5_150_000, 5_895_000);

impl Spectrum {
    pub fn new(range_khz: (u64, u64), need: Need, emits: Emits) -> Self {
        Self {
            low_khz: range_khz.0,
            high_khz: range_khz.1,
            need,
            emits,
        }
    }

    /// An IEEE 802.15.4 channel (11-26) in the 2.4 GHz band: 2 MHz wide.
    pub fn ieee802154(channel: u8, need: Need, emits: Emits) -> Self {
        let center = 2_405_000 + 5_000 * (u64::from(channel).saturating_sub(11));
        Self::new((center - 1_000, center + 1_000), need, emits)
    }

    /// The claims of this range: the range and the coarse air claim.
    pub fn claims(&self) -> [Claim; 2] {
        let coarse = if self.need == Need::Strict || self.emits == Emits::Noisy {
            Mode::Exclusive
        } else {
            Mode::Shared
        };
        [
            Claim::shared(self.resource()),
            Claim {
                resource: AIR.to_owned(),
                mode: coarse,
            },
        ]
    }

    fn resource(&self) -> String {
        format!(
            "{PREFIX}{}-{}:{}:{}",
            self.low_khz,
            self.high_khz,
            match self.need {
                Need::None => "none",
                Need::Tolerant => "tolerant",
                Need::Strict => "strict",
            },
            match self.emits {
                Emits::None => "none",
                Emits::Normal => "normal",
                Emits::Noisy => "noisy",
            }
        )
    }

    pub(crate) fn parse(resource: &str) -> Option<Self> {
        let rest = resource.strip_prefix(PREFIX)?;
        let mut parts = rest.split(':');
        let (low, high) = parts.next()?.split_once('-')?;
        let need = match parts.next()? {
            "none" => Need::None,
            "tolerant" => Need::Tolerant,
            "strict" => Need::Strict,
            _ => return None,
        };
        let emits = match parts.next()? {
            "none" => Emits::None,
            "normal" => Emits::Normal,
            "noisy" => Emits::Noisy,
            _ => return None,
        };
        parts.next().is_none().then_some(())?;
        Some(Self {
            low_khz: low.parse().ok()?,
            high_khz: high.parse().ok()?,
            need,
            emits,
        })
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.low_khz <= other.high_khz && other.low_khz <= self.high_khz
    }

    /// Whether this range and `other` cannot be used at once.
    pub(crate) fn conflicts(&self, other: &Self) -> bool {
        let excludes = |needs: &Self, emits: &Self| match needs.need {
            Need::Strict => emits.emits != Emits::None,
            Need::Tolerant => emits.emits == Emits::Noisy,
            Need::None => false,
        };
        self.overlaps(other) && (excludes(self, other) || excludes(other, self))
    }

    /// Whether a lease holding this range may run work using `inner`.
    pub(crate) fn covers(&self, inner: &Self) -> bool {
        self.low_khz <= inner.low_khz
            && inner.high_khz <= self.high_khz
            && self.need >= inner.need
            && self.emits >= inner.emits
    }
}

impl std::fmt::Display for Spectrum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mhz = |khz: u64| {
            if khz.is_multiple_of(1000) {
                format!("{}", khz / 1000)
            } else {
                format!("{:.1}", khz as f64 / 1000.0)
            }
        };
        write!(
            f,
            "air {}-{} MHz ({:?} need, {:?} emission)",
            mhz(self.low_khz),
            mhz(self.high_khz),
            self.need,
            self.emits
        )
    }
}

/// The ranges among `claims`.
pub fn ranges(claims: &[Claim]) -> Vec<Spectrum> {
    claims
        .iter()
        .filter_map(|claim| Spectrum::parse(&claim.resource))
        .collect()
}

pub fn is_range(resource: &str) -> bool {
    resource.starts_with(PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(need: Need, emits: Emits) -> Spectrum {
        Spectrum::new(BAND_2G4, need, emits)
    }

    #[test]
    fn a_range_round_trips_through_its_resource() {
        let range = Spectrum::ieee802154(15, Need::Tolerant, Emits::Noisy);
        assert_eq!((range.low_khz, range.high_khz), (2_424_000, 2_426_000));
        let [claim, air] = range.claims();
        assert_eq!(Spectrum::parse(&claim.resource), Some(range));
        assert_eq!(claim.mode, Mode::Shared);
        assert_eq!(
            air,
            Claim::exclusive(AIR),
            "noisy work excludes older arbiters' air"
        );
        assert!(Spectrum::parse("spectrum:1-2:loud:none").is_none());
    }

    #[test]
    fn needs_exclude_transmissions_only_where_ranges_overlap() {
        use Emits as E;
        use Need as N;
        let calibration = band(N::Strict, E::Normal);
        let quiet_listener = band(N::Strict, E::None);
        let gatt = band(N::Tolerant, E::Normal);
        let peer_exchange = Spectrum::ieee802154(15, N::Tolerant, E::Normal);
        let air_check = Spectrum::ieee802154(15, N::Tolerant, E::Noisy);
        let five = Spectrum::new(BAND_5G, N::Strict, E::Noisy);

        assert!(
            calibration.conflicts(&gatt),
            "strict excludes any transmission"
        );
        assert!(
            !quiet_listener.conflicts(&band(N::Strict, E::None)),
            "listeners coexist"
        );
        assert!(!gatt.conflicts(&peer_exchange), "tolerant work coexists");
        assert!(gatt.conflicts(&air_check), "tolerant excludes noisy");
        assert!(!five.conflicts(&calibration), "disjoint bands");
        assert!(
            !Spectrum::ieee802154(11, N::Strict, E::Normal).conflicts(&peer_exchange),
            "disjoint channels"
        );
    }

    #[test]
    fn a_wider_and_stronger_range_covers_a_narrower_one() {
        let outer = band(Need::Strict, Emits::Noisy);
        assert!(outer.covers(&Spectrum::ieee802154(15, Need::Tolerant, Emits::Normal)));
        assert!(!band(Need::Tolerant, Emits::Normal).covers(&outer));
    }
}
