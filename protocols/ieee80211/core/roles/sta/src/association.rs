//! Association preference among modes admitted by both local and peer profiles.
//!
//! The hardware profile decides eligibility, including supported MCS and width.
//! This policy neither advertises capabilities nor encodes a PHY channel command.

use oer_ieee80211_core_upper_mac::phy_mode::PhyMode;
use oer_ieee80211_core_upper_mac::protection::{
    BasicRates, BssProtection, HePacketPadding, HeTxopDurationRtsThreshold,
};
use oer_ieee80211_wire::management::CapabilityInformation;

/// Caller policy applied before constructing one STA association request.
///
/// These are preferences rather than unchecked PHY claims. `PreferHe20`
/// still falls back when the peer does not advertise the complete HE20 MCS9
/// contract; `ForceHt20` is retained as the diagnostic negative-control mode
/// and may subsequently be rejected by
/// [`AssociationRequest::encode`](oer_ieee80211_wire::station::AssociationRequest::encode).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Preference {
    /// Select among the mutually supported modes.
    Automatic,
    /// Prefer HE20 when the peer admits it.
    PreferHe20,
    /// Require the diagnostic HT20 selection.
    ForceHt20,
}

/// Choose a mode after the caller has intersected local and peer capabilities.
/// A peer without HT is legacy; HT20 is the baseline of one with HT; a later
/// association encoder may still reject the peer.
pub fn select_phy(
    preference: Preference,
    ht_available: bool,
    ht40_available: bool,
    he20_available: bool,
) -> PhyMode {
    if !ht_available {
        PhyMode::Legacy
    } else if preference == Preference::PreferHe20 && he20_available {
        PhyMode::He20
    } else if preference == Preference::ForceHt20 {
        PhyMode::Ht20
    } else if ht40_available {
        PhyMode::Ht40
    } else if he20_available {
        PhyMode::He20
    } else {
        PhyMode::Ht20
    }
}

/// The ERP protection the access point's ERP elements ask for. A BSS sends
/// one ERP element; repeats are malformed, and protection fails closed while
/// the rest of the record stays usable: every bit any of them asserts counts,
/// and elements beyond the record's capacity, which the station did not see,
/// count as asserting Use Protection and long preambles.
pub fn erp_protection(
    access_point: &oer_ieee80211_wire::scan::ScanRecord,
) -> oer_ieee80211_wire::protection::ErpProtection {
    use oer_ieee80211_wire::protection::ErpProtection;
    let stored = access_point.erp_information();
    if usize::from(access_point.erp_element_count()) > stored.len() {
        return ErpProtection::new(true, true);
    }
    ErpProtection::from_information(stored.iter().copied().reduce(|merged, next| merged | next))
}

/// The mode a station associates with `access_point` in under `preference`:
/// HE20 when the access point's HE Capabilities admit bidirectional MCS 9
/// and its HE Operation parses, HT40 when its HT Capabilities and HT
/// Operation agree on a usable secondary channel and the station tunes
/// 40 MHz (`ht40_capable`), HT20 otherwise, and legacy for an access point
/// without HT Capabilities.
pub fn select_association_phy(
    access_point: &oer_ieee80211_wire::scan::ScanRecord,
    channel: u8,
    preference: Preference,
    ht40_capable: bool,
) -> PhyMode {
    let he20_available =
        oer_ieee80211_wire::he::parse_he20_capabilities(access_point.he_capability_ie_bytes())
            .is_ok_and(|capability| capability.supports_bidirectional_mcs9())
            && oer_ieee80211_wire::he::parse_he20_operation(access_point.he_operation_ie_bytes())
                .is_ok();
    select_phy(
        preference,
        access_point.ht_capability_ie_bytes().is_some(),
        ht40_capable && access_point.ht40_secondary_channel(channel).is_some(),
        he20_available,
    )
}

/// What an associated station knows of its access point: the PHY it
/// associated with and the peer facts its transmitter follows, from the
/// access point's scan record and its Association Response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAssociatedPeer {
    pub phy: PhyMode,
    pub legacy_rates: oer_ieee80211_core_upper_mac::rate_control::NegotiatedLegacyRates,
    pub ht_capabilities: Option<oer_ieee80211_wire::ht::HtPeerCapabilities>,
    /// The A-MPDU Parameters field of the access point's HT Capabilities:
    /// its maximum A-MPDU length exponent and minimum MPDU start spacing;
    /// zero without HT Capabilities.
    pub ht_ampdu_parameters: u8,
    pub he_capabilities: Option<oer_ieee80211_wire::he::He20Capabilities>,
    /// The HE state of an HE association.
    pub he_peer_state: Option<oer_ieee80211_wire::he::He20PeerState>,
    /// The BSS color of the access point's HE Operation, zero without one.
    pub he_bss_color: u8,
    /// The protection facts of the BSS the transmitter's planner applies.
    pub protection: oer_ieee80211_core_upper_mac::protection::BssProtection,
    /// The EDCA parameters the station's interface contends with: those
    /// of the Association Response's WMM Parameter element, else those the
    /// scan heard, else the standard defaults of a QoS BSS, or the DCF's
    /// in a BSS without WMM.
    pub edca: oer_ieee80211_wire::extensions::wmm::WmmParameterSet,
}

/// Why the associated peer could not be derived.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssociatedPeerError {
    /// The Association Response refused the association with this status.
    Rejected(u16),
    /// The association contains no usable non-HT rate for control/fallback.
    NoLegacyRates,
    /// An HE association whose access point's HE Capabilities do not parse.
    HeCapabilities(oer_ieee80211_wire::he::HeElementError),
    /// An HE association whose access point's HE elements give no peer state.
    HePeer(oer_ieee80211_wire::he::HeElementError),
}

impl StaAssociatedPeer {
    /// What the station's rate control knows of its access point.
    pub const fn rate_peer(&self) -> oer_ieee80211_core_upper_mac::rate_control::RatePeer {
        oer_ieee80211_core_upper_mac::rate_control::RatePeer {
            phy: self.phy,
            legacy_rates: self.legacy_rates,
            ht_capabilities: self.ht_capabilities,
            he_capabilities: self.he_capabilities,
            he_peer_state: self.he_peer_state,
        }
    }

    /// The peer `access_point` became by `response` to an association with
    /// `phy`. `he_packet_padding` reads the nominal packet padding from the
    /// access point's HE Capabilities element, an integrator's policy (the
    /// Espressif stack's is `oer-espressif-ieee80211-policy`'s).
    pub fn derive(
        access_point: &oer_ieee80211_wire::scan::ScanRecord,
        response: &oer_ieee80211_wire::station::AssociationResponse,
        phy: PhyMode,
        he_packet_padding: impl FnOnce(
            &[u8],
        )
            -> oer_ieee80211_core_upper_mac::protection::HePacketPadding,
    ) -> Result<Self, StaAssociatedPeerError> {
        use oer_ieee80211_wire::{
            extensions::wmm::WmmParameterSet,
            he::{parse_he20_capabilities, parse_he20_operation, parse_he20_peer_state},
            protection::HtProtectionMode,
        };
        if response.status_code != 0 {
            return Err(StaAssociatedPeerError::Rejected(response.status_code));
        }
        let he_capability = access_point.he_capability_ie_bytes();
        let (he_capabilities, he_peer_state) = if phy == PhyMode::He20 {
            let capabilities = parse_he20_capabilities(he_capability)
                .map_err(StaAssociatedPeerError::HeCapabilities)?;
            let state = parse_he20_peer_state(he_capability, access_point.he_operation_ie_bytes())
                .map_err(StaAssociatedPeerError::HePeer)?;
            (Some(capabilities), Some(state))
        } else {
            (parse_he20_capabilities(he_capability).ok(), None)
        };
        let edca = match response
            .wmm_parameters
            .or_else(|| access_point.wmm_parameters())
        {
            Some(parameters) => parameters,
            None if access_point.wmm_ie_bytes().is_empty() => WmmParameterSet::DCF,
            None => WmmParameterSet::STANDARD_DEFAULT,
        };
        Ok(Self {
            phy,
            legacy_rates:
                oer_ieee80211_core_upper_mac::rate_control::NegotiatedLegacyRates::from_rate_elements(
                    access_point.supported_rates_bytes(),
                    access_point.extended_supported_rates_bytes(),
                )
                .ok_or(StaAssociatedPeerError::NoLegacyRates)?,
            ht_capabilities: access_point.ht_peer_capabilities(),
            // HT Capabilities is kept whole: payload byte two is element
            // byte four.
            ht_ampdu_parameters: access_point
                .ht_capability_ie_bytes()
                .map_or(0, |capability| capability[4]),
            he_capabilities,
            he_peer_state,
            he_bss_color: parse_he20_operation(access_point.he_operation_ie_bytes())
                .map_or(0, |operation| operation.effective_bss_color()),
            protection: BssProtection {
                erp: erp_protection(access_point),
                ht: HtProtectionMode::from_operation_ie(access_point.ht_operation_ie_bytes()),
                he_txop_rts_threshold: he_peer_state
                    .and_then(|state| state.rts_threshold)
                    .and_then(HeTxopDurationRtsThreshold::new),
                he_packet_padding: if he_capability.is_empty() {
                    HePacketPadding::None
                } else {
                    he_packet_padding(he_capability)
                },
                basic_rates: BasicRates::from_rate_elements(
                    access_point.supported_rates_bytes(),
                    access_point.extended_supported_rates_bytes(),
                ),
                short_preamble: CapabilityInformation::from_bytes(
                    access_point.capability_info.to_le_bytes(),
                )
                .short_preamble(),
            },
            edca,
        })
    }
}

#[cfg(test)]
mod tests;
