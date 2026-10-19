//! The image a scenario needs: its chip, its composition and the agent
//! capabilities it must serve.

use core::fmt;
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{CapabilityKey, ChipId, CompositionName};

/// The product composition an image is built on. The HIL image composes
/// the same crate as the product; only the observer parameter and the
/// agent's capability servers differ.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case", tag = "composition", content = "crate")]
pub enum Composition {
    /// A platform scenario, such as boot smoke, that any composition of
    /// its chip serves.
    Any,
    Product(CompositionName),
}

/// The chip, composition and agent capabilities an image must serve.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TargetCapabilities {
    pub chip: ChipId,
    pub composition: Composition,
    pub capabilities: BTreeSet<CapabilityKey>,
}

/// Two scenarios need different images: other chips, or other product
/// compositions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImageConflict {
    Chips(ChipId, ChipId),
    Compositions(CompositionName, CompositionName),
}

impl fmt::Display for ImageConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Chips(a, b) => write!(f, "chips `{a}` and `{b}` cannot share an image"),
            Self::Compositions(a, b) => {
                write!(f, "compositions `{a}` and `{b}` cannot share an image")
            }
        }
    }
}

impl std::error::Error for ImageConflict {}

impl TargetCapabilities {
    /// The image serving both: one chip, one composition, the union of
    /// capabilities.
    pub fn union(&self, other: &Self) -> Result<Self, ImageConflict> {
        if self.chip != other.chip {
            return Err(ImageConflict::Chips(self.chip.clone(), other.chip.clone()));
        }
        let composition = match (&self.composition, &other.composition) {
            (Composition::Product(a), Composition::Product(b)) if a != b => {
                return Err(ImageConflict::Compositions(a.clone(), b.clone()));
            }
            (Composition::Product(a), _) | (Composition::Any, Composition::Product(a)) => {
                Composition::Product(a.clone())
            }
            (Composition::Any, Composition::Any) => Composition::Any,
        };
        Ok(Self {
            chip: self.chip.clone(),
            composition,
            capabilities: self
                .capabilities
                .union(&other.capabilities)
                .cloned()
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name;

    fn of(chip: ChipId, composition: Composition, keys: &[CapabilityKey]) -> TargetCapabilities {
        TargetCapabilities {
            chip,
            composition,
            capabilities: keys.iter().cloned().collect(),
        }
    }

    #[test]
    fn one_image_serves_the_union_over_one_chip_and_composition() {
        let s31 = name!(ChipId, "esp32s31");
        let station = Composition::Product(name!(CompositionName, "oer-esp32s31-ieee80211-system"));
        let smoke = of(
            s31.clone(),
            Composition::Any,
            &[name!(CapabilityKey, "base.boot")],
        );
        let conformance = of(
            s31.clone(),
            station.clone(),
            &[name!(CapabilityKey, "conformance.lower-mac")],
        );
        let union = smoke.union(&conformance).unwrap();
        assert_eq!(union.composition, station);
        assert_eq!(union.capabilities.len(), 2);
        let ble = of(
            s31,
            Composition::Product(name!(CompositionName, "toy-bluetooth-system")),
            &[],
        );
        assert!(matches!(
            union.union(&ble),
            Err(ImageConflict::Compositions(..))
        ));
    }

    #[test]
    fn platform_scenarios_of_two_chips_need_two_images() {
        let boot = [name!(CapabilityKey, "base.boot")];
        let s31 = of(name!(ChipId, "esp32s31"), Composition::Any, &boot);
        let c5 = of(name!(ChipId, "esp32c5"), Composition::Any, &boot);
        assert!(matches!(s31.union(&c5), Err(ImageConflict::Chips(..))));
    }
}
