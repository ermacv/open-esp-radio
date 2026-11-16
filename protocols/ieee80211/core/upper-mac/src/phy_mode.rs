//! The PHY of one association, its format and its channel width, and the
//! elements its request carries.
use oer_protocols_ieee80211_contracts_port::{PhyFormat, Width};
use oer_protocols_ieee80211_wire::station::{
    AssociationCapabilities, AssociationPhyElements, HeUlMuPowerCapability, StaPowerCapability,
};

/// The PHY of one association: a format and a channel width.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyMode {
    pub format: PhyFormat,
    pub width: Width,
}

/// The PHY a join decided with its access point, before its request: what
/// the request carries follows from it, an HE one with its power elements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NegotiatedPhy {
    /// Non-HT, at 20 MHz.
    NonHt,
    /// HT at `width`.
    Ht { width: Width },
    /// HE at 20 MHz, with the HE Power Capability and the HE UL-MU power
    /// capability its request carries.
    He {
        power_capability: StaPowerCapability,
        he_ul_mu_power: HeUlMuPowerCapability,
    },
}

impl NegotiatedPhy {
    /// The association's format and width.
    pub const fn mode(self) -> PhyMode {
        match self {
            Self::NonHt => PhyMode {
                format: PhyFormat::NonHt,
                width: Width::Mhz20,
            },
            Self::Ht { width } => PhyMode {
                format: PhyFormat::Ht,
                width,
            },
            Self::He { .. } => PhyMode {
                format: PhyFormat::He,
                width: Width::Mhz20,
            },
        }
    }

    /// The elements the request carries: an HT request the HT element of its
    /// width; an HE one the HT element beside HE, its HE element and its
    /// power elements.
    pub const fn elements(
        self,
        capabilities: &AssociationCapabilities,
    ) -> AssociationPhyElements<'_> {
        match self {
            Self::NonHt => AssociationPhyElements::NonHt,
            Self::Ht { width } => AssociationPhyElements::Ht {
                ht_capability: match width {
                    Width::Mhz20 => &capabilities.ht.mhz20,
                    Width::Mhz40 => &capabilities.ht.mhz40,
                },
            },
            Self::He {
                power_capability,
                he_ul_mu_power,
            } => AssociationPhyElements::He {
                ht_capability: &capabilities.ht_with_he,
                he_capability: &capabilities.he,
                power_capability,
                he_ul_mu_power,
            },
        }
    }
}

#[cfg(test)]
mod tests;
