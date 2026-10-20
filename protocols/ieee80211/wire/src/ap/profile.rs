//! Caller-selected AP advertisements, independent of silicon and peer state.
//!
//! Fixed record capacities preserve the implemented beacon/association format.
//! The chip profile chooses values; these types validate and encode them.
//! WMM advertisements use the extension's wire codec and the same typed
//! category selector as received parameter sets.

use crate::{
    extensions::wmm::{
        WMM_NIBBLE_MAX, WMM_PARAMETER_ELEMENT_LEN, WmmAcParameters, encode_wmm_parameter_element,
    },
    ht::HtLocalCapabilities,
    qos::WmmAccessCategory,
    security::LinkProtection,
};

/// The Supported Rates and Extended Supported Rates records of this AP.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyRates {
    supported: [u8; 8],
    extended: [u8; 4],
}

impl LegacyRates {
    /// Rates use 500-kbit/s units, with bit seven marking a basic rate.
    pub const fn new(supported: [u8; 8], extended: [u8; 4]) -> Self {
        Self {
            supported,
            extended,
        }
    }

    pub const fn supported(&self) -> &[u8; 8] {
        &self.supported
    }

    pub const fn extended(&self) -> &[u8; 4] {
        &self.extended
    }

    pub fn supports(&self, rate_500kbps: u8) -> bool {
        self.supported
            .iter()
            .chain(self.extended.iter())
            .any(|encoded| crate::phy::SupportedRate::from_bytes([*encoded]).rate() == rate_500kbps)
    }
}

/// Advertised WMM access parameters in standard ACI order: BE, BK, VI, VO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WmmParameters {
    parameter_set_count: u8,
    uapsd: bool,
    access_categories: [WmmAcParameters; 4],
}

impl WmmParameters {
    /// The parameters, when each field fits its four bits: `None`
    /// otherwise. A `const` caller checks them at build.
    pub const fn new(
        parameter_set_count: u8,
        uapsd: bool,
        access_categories: [WmmAcParameters; 4],
    ) -> Option<Self> {
        if parameter_set_count > WMM_NIBBLE_MAX {
            return None;
        }
        let mut remaining = access_categories.as_slice();
        while let Some((category, rest)) = remaining.split_first() {
            if category.aifsn > WMM_NIBBLE_MAX
                || category.ecw_min > WMM_NIBBLE_MAX
                || category.ecw_max > WMM_NIBBLE_MAX
            {
                return None;
            }
            remaining = rest;
        }
        Some(Self {
            parameter_set_count,
            uapsd,
            access_categories,
        })
    }

    /// The advertised parameters of `category`, which the access point's
    /// own transmissions also follow.
    pub const fn access_category(self, category: WmmAccessCategory) -> WmmAcParameters {
        *category.select(&self.access_categories)
    }

    /// Encode a complete vendor-specific WMM Parameter Element.
    pub const fn element(self) -> [u8; WMM_PARAMETER_ELEMENT_LEN] {
        encode_wmm_parameter_element(self.parameter_set_count, self.uapsd, self.access_categories)
    }
}

/// One coherent local advertisement used by AP discovery and association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Advertisement {
    pub legacy_rates: LegacyRates,
    pub ht: HtLocalCapabilities,
    pub wmm: WmmParameters,
    capability_information: u16,
}

impl Advertisement {
    /// Privacy belongs to the selected security mode, not the hardware profile.
    pub const fn new(
        legacy_rates: LegacyRates,
        ht: HtLocalCapabilities,
        wmm: WmmParameters,
        capability_information: u16,
    ) -> Option<Self> {
        // Privacy belongs to the security mode: `None` when it is set.
        if crate::management::CapabilityInformation::from_bytes(
            capability_information.to_le_bytes(),
        )
        .privacy()
        {
            return None;
        }
        Some(Self {
            legacy_rates,
            ht,
            wmm,
            capability_information,
        })
    }

    pub const fn capabilities(&self, security: LinkProtection) -> u16 {
        u16::from_le_bytes(
            crate::management::CapabilityInformation::from_bytes(
                self.capability_information.to_le_bytes(),
            )
            .with_privacy(matches!(security, LinkProtection::Ccmp))
            .to_bytes(),
        )
    }
}

#[cfg(test)]
pub(crate) mod tests;
