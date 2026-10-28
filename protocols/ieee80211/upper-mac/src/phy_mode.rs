//! The PHY mode of one association and the elements it requests.
use oer_ieee80211_wire::{
    he::{parse_he20_capabilities, parse_he20_operation},
    scan::ScanRecord,
    station::association::{
        AssociationCapabilities, AssociationElements, AssociationPhy, AssociationRequestError,
        HeUlMuPowerCapability, StaPowerCapability,
    },
};

/// PHY mode selected for one association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyMode {
    /// Non-HT operation.
    Legacy,
    /// HT on a 20 MHz channel.
    Ht20,
    /// HT on a 40 MHz channel.
    Ht40,
    /// HE on a 20 MHz channel.
    He20,
}

impl PhyMode {
    /// The selected channel bandwidth, in MHz.
    pub const fn bandwidth_mhz(self) -> u8 {
        match self {
            Self::Ht40 => 40,
            Self::Legacy | Self::Ht20 | Self::He20 => 20,
        }
    }

    /// Stable diagnostic name of the mode.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Ht20 => "ht20",
            Self::Ht40 => "ht40",
            Self::He20 => "he20",
        }
    }
}

impl AssociationPhy for PhyMode {
    fn elements<'a>(
        &self,
        access_point: &ScanRecord,
        capabilities: &'a AssociationCapabilities,
        power_capability: Option<StaPowerCapability>,
        he_ul_mu_power: Option<HeUlMuPowerCapability>,
    ) -> Result<AssociationElements<'a>, AssociationRequestError> {
        let (ht_capability, he_capability, selected_power, selected_ul_mu) = match *self {
            PhyMode::Legacy => (None, None, None, None),
            PhyMode::Ht20 if access_point.ht_capability_ie_bytes().is_some() => {
                (Some(&capabilities.ht20), None, None, None)
            }
            PhyMode::Ht20 => {
                return Err(AssociationRequestError::HtUnsupportedByAccessPoint);
            }
            PhyMode::Ht40 if access_point.ht40_secondary_channel().is_some() => {
                (Some(&capabilities.ht40), None, None, None)
            }
            PhyMode::Ht40 => {
                return Err(AssociationRequestError::Ht40UnsupportedByAccessPoint);
            }
            PhyMode::He20
                if access_point.ht_capability_ie_bytes().is_some()
                    && parse_he20_capabilities(access_point.he_capability_ie_bytes())
                        .is_ok_and(|capability| capability.supports_bidirectional_mcs9())
                    && parse_he20_operation(access_point.he_operation_ie_bytes()).is_ok() =>
            {
                (
                    Some(&capabilities.he20_ht),
                    Some(&capabilities.he20),
                    Some(power_capability.ok_or(AssociationRequestError::MissingPowerCapability)?),
                    Some(
                        he_ul_mu_power
                            .ok_or(AssociationRequestError::MissingHeUlMuPowerCapability)?,
                    ),
                )
            }
            PhyMode::He20 => {
                return Err(AssociationRequestError::He20UnsupportedByAccessPoint);
            }
        };
        if *self != PhyMode::He20 {
            if power_capability.is_some() {
                return Err(AssociationRequestError::UnexpectedPowerCapability);
            }
            if he_ul_mu_power.is_some() {
                return Err(AssociationRequestError::UnexpectedHeUlMuPowerCapability);
            }
        }
        Ok(AssociationElements {
            ht_capability,
            he_capability,
            power_capability: selected_power,
            he_ul_mu_power: selected_ul_mu,
        })
    }
}

#[cfg(test)]
mod tests;
