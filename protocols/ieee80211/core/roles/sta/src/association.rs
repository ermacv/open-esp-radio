//! Association preference among modes admitted by both local and peer profiles.
//!
//! The hardware profile decides eligibility, including supported MCS and width.
//! This policy neither advertises capabilities nor encodes a PHY channel command.

use oer_protocols_ieee80211_core_upper_mac::phy_mode::PhyMode;
use oer_protocols_ieee80211_core_upper_mac::protection::{
    BasicRates, BssProtection, HePacketPadding, HeTxopDurationRtsThreshold,
};
use oer_protocols_ieee80211_wire::management::CapabilityInformation;

/// Caller policy applied before constructing one STA association request.
///
/// These are preferences rather than unchecked PHY claims. `PreferHe20`
/// still falls back when the peer does not advertise the complete HE20 MCS9
/// contract; `ForceHt20` is retained as the diagnostic negative-control mode
/// and may subsequently be rejected by
/// [`AssociationRequest::encode`](oer_protocols_ieee80211_wire::station::AssociationRequest::encode).
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

/// The ERP protection one frame's ERP elements ask for: the station's one
/// fold for scan results and beacons alike. A BSS sends one ERP element;
/// repeats are malformed, and protection fails closed while the rest of the
/// frame stays usable: every bit any of them asserts counts. No element asks
/// for nothing. Elements that cannot be read ask for Use Protection and long
/// preambles. Each frame decides on its own; a later frame's elements
/// replace an earlier one's.
pub fn erp_protection(
    elements: Result<
        oer_protocols_ieee80211_wire::protection::ErpElements<'_>,
        oer_protocols_ieee80211_wire::protection::ErpElementsError,
    >,
) -> oer_protocols_ieee80211_wire::protection::ErpProtection {
    use oer_protocols_ieee80211_wire::protection::ErpProtection;
    match elements {
        Ok(elements) => ErpProtection::from_information(
            elements.information().reduce(|merged, next| merged | next),
        ),
        Err(_) => ErpProtection::new(true, true),
    }
}

/// The mode a station associates with `access_point` in under `preference`:
/// HE20 when the access point's HE Capabilities admit bidirectional MCS 9
/// and its HE Operation parses, HT40 when its HT Capabilities and HT
/// Operation agree on a usable secondary channel and the station tunes
/// 40 MHz (`ht40_capable`), HT20 otherwise, and legacy for an access point
/// without HT Capabilities.
pub fn select_association_phy(
    access_point: &oer_protocols_ieee80211_wire::scan::ScanRecord,
    channel: u8,
    preference: Preference,
    ht40_capable: bool,
) -> PhyMode {
    select_phy(
        preference,
        access_point.ht_capability_ie_bytes().is_some(),
        ht40_capable && access_point.ht40_secondary_channel(channel).is_some(),
        he20_admitted(access_point),
    )
}

/// Whether `access_point` admits an HE20 association: it advertises HT
/// Capabilities, HE Capabilities with HE-MCS 0..9 both ways on one stream
/// and an HE Operation element that parses. The same rule admits the HE20
/// request's elements ([`PhyMode`]'s `AssociationPhy`).
pub fn he20_admitted(access_point: &oer_protocols_ieee80211_wire::scan::ScanRecord) -> bool {
    access_point.ht_capability_ie_bytes().is_some()
        && oer_protocols_ieee80211_wire::he::parse_he20_capabilities(
            access_point.he_capability_ie_bytes(),
        )
        .is_ok_and(|capability| capability.supports_bidirectional_mcs9())
        && oer_protocols_ieee80211_wire::he::parse_he20_operation(
            access_point.he_operation_ie_bytes(),
        )
        .is_ok()
}

/// The power elements an HE20 Association Request carries: the Power
/// Capability and the HE UL-MU power capability, from the composition's
/// calibrated transmit power.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaHePower {
    pub power_capability: oer_protocols_ieee80211_wire::station::StaPowerCapability,
    pub ul_mu_power: oer_protocols_ieee80211_wire::station::HeUlMuPowerCapability,
}

/// The PHY a station asks its access point for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPhyRequest {
    /// Non-HT.
    Legacy,
    /// HT20. A request to an access point without HT does not encode.
    Ht20,
    /// HT40 on one of the station's 40 MHz channels where the access
    /// point's HT elements admit it, else HT20 with an access point that
    /// advertises HT, else non-HT.
    Ht40,
    /// HE20 with an access point that admits it ([`he20_admitted`]), with
    /// `power`'s elements; with any other access point the request
    /// [`Self::Ht40`] names when `ht40` holds, else [`Self::Ht20`].
    PreferHe20 { ht40: bool, power: StaHePower },
}

impl StaPhyRequest {
    /// This request without HE20: [`Self::PreferHe20`]'s fallback,
    /// [`Self::Ht40`] when `ht40` holds, else [`Self::Ht20`]; any other
    /// request unchanged.
    pub const fn without_he20(self) -> Self {
        match self {
            Self::PreferHe20 { ht40: true, .. } => Self::Ht40,
            Self::PreferHe20 { ht40: false, .. } => Self::Ht20,
            other => other,
        }
    }
}

/// What an associated station knows of its access point: the PHY it
/// associated with and the peer facts its transmitter follows, from the
/// access point's scan record and its Association Response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAssociatedPeer {
    pub phy: PhyMode,
    pub legacy_rates: oer_protocols_ieee80211_core_upper_mac::rate_control::NegotiatedLegacyRates,
    pub ht_capabilities: Option<oer_protocols_ieee80211_wire::ht::HtPeerCapabilities>,
    /// The A-MPDU Parameters field of the access point's HT Capabilities:
    /// its maximum A-MPDU length exponent and minimum MPDU start spacing;
    /// zero without HT Capabilities.
    pub ht_ampdu_parameters: u8,
    pub he_capabilities: Option<oer_protocols_ieee80211_wire::he::He20Capabilities>,
    /// The HE state of an HE association.
    pub he_peer_state: Option<oer_protocols_ieee80211_wire::he::He20PeerState>,
    /// The BSS color of the access point's HE Operation, zero without one.
    pub he_bss_color: u8,
    /// What the station's MAC follows of an HE association: its peer
    /// state, the AID and the access point's minimum MPDU start spacing.
    pub he_association: Option<oer_protocols_ieee80211_contracts_port::He20Association>,
    /// The protection facts of the BSS the transmitter's planner applies.
    pub protection: oer_protocols_ieee80211_core_upper_mac::protection::BssProtection,
    /// The EDCA parameters the station's interface contends with: those
    /// of the Association Response's WMM Parameter element, else those the
    /// scan heard, else the standard defaults of a QoS BSS, or the DCF's
    /// in a BSS without WMM.
    pub edca: oer_protocols_ieee80211_wire::extensions::wmm::WmmParameterSet,
}

/// Why the associated peer could not be derived.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssociatedPeerError {
    /// The Association Response refused the association with this status.
    Rejected(u16),
    /// The association contains no usable non-HT rate for control/fallback.
    NoLegacyRates,
    /// An HE association whose Association Response's HE Capabilities do
    /// not parse.
    HeCapabilities(oer_protocols_ieee80211_wire::he::HeElementError),
    /// An HE association whose Association Response's HE Operation does not
    /// parse.
    HeOperation(oer_protocols_ieee80211_wire::he::HeElementError),
    /// An HE association whose Association Response's HE elements give no
    /// peer state.
    HePeer(oer_protocols_ieee80211_wire::he::HeElementError),
    /// An HE association whose Association Response carries no HE
    /// Capabilities or no HE Operation element: the access point did not
    /// associate the station as an HE station.
    HeNotGranted,
    /// An HE association whose Association Response's HE Capabilities no
    /// longer admit HE20 (HE-MCS 0..9 both ways on one stream): a change from
    /// the scanned access point the station does not take.
    HeUnsupported,
    /// An HE association whose Association Response repeats an HE element or
    /// carries one longer than the station keeps.
    HeElementsUnusable,
    /// An HE association whose AID or HE parameters lie outside their
    /// fields.
    HeAssociation(oer_protocols_ieee80211_contracts_port::He20AssociationError),
}

impl StaAssociatedPeerError {
    /// Whether the access point did not set up the HE association the
    /// station asked for.
    pub const fn he(self) -> bool {
        matches!(
            self,
            Self::HeCapabilities(_)
                | Self::HeOperation(_)
                | Self::HePeer(_)
                | Self::HeNotGranted
                | Self::HeUnsupported
                | Self::HeElementsUnusable
                | Self::HeAssociation(_)
        )
    }
}

impl StaAssociatedPeer {
    /// What the station's rate control knows of its access point.
    pub const fn rate_peer(
        &self,
    ) -> oer_protocols_ieee80211_core_upper_mac::rate_control::RatePeer {
        oer_protocols_ieee80211_core_upper_mac::rate_control::RatePeer {
            phy: self.phy,
            legacy_rates: self.legacy_rates,
            ht_capabilities: self.ht_capabilities,
            he_capabilities: self.he_capabilities,
            he_peer_state: self.he_peer_state,
        }
    }

    /// The peer `access_point`, whose ERP elements ask for `erp`, became by
    /// `response` to an association with `phy`. An HE20 association takes
    /// its HE facts from the response's HE elements, which must admit HE20
    /// as the scanned ones did; the scan's HE elements serve other modes. `he_packet_padding` reads the nominal packet padding from the
    /// access point's HE Capabilities element, an integrator's policy (the
    /// Espressif stack's is `oer-chips-family-radio-ieee80211-policy`'s).
    pub fn derive(
        access_point: &oer_protocols_ieee80211_wire::scan::ScanRecord,
        erp: oer_protocols_ieee80211_wire::protection::ErpProtection,
        response: &oer_protocols_ieee80211_wire::station::AssociationResponse,
        phy: PhyMode,
        he_packet_padding: impl FnOnce(
            &[u8],
        )
            -> oer_protocols_ieee80211_core_upper_mac::protection::HePacketPadding,
    ) -> Result<Self, StaAssociatedPeerError> {
        use oer_protocols_ieee80211_wire::{
            extensions::wmm::WmmParameterSet,
            he::{parse_he20_capabilities, parse_he20_operation, parse_he20_peer_state},
            protection::HtProtectionMode,
        };
        if response.status_code != 0 {
            return Err(StaAssociatedPeerError::Rejected(response.status_code));
        }
        // The A-MPDU Parameters field of the access point's HT Capabilities
        // (kept whole: payload byte two is element byte four).
        let ht_ampdu_parameters = access_point
            .ht_capability_ie_bytes()
            .map_or(0, |capability| capability[4]);
        let (he_capability, he_capabilities, he_peer_state, he_bss_color) = if phy == PhyMode::He20
        {
            let he = &response.he;
            if he.unusable() {
                return Err(StaAssociatedPeerError::HeElementsUnusable);
            }
            let (capability, operation) = (he.capability(), he.operation());
            if capability.is_empty() || operation.is_empty() {
                return Err(StaAssociatedPeerError::HeNotGranted);
            }
            let capabilities = parse_he20_capabilities(capability)
                .map_err(StaAssociatedPeerError::HeCapabilities)?;
            if !capabilities.supports_bidirectional_mcs9() {
                return Err(StaAssociatedPeerError::HeUnsupported);
            }
            let bss_color = parse_he20_operation(operation)
                .map_err(StaAssociatedPeerError::HeOperation)?
                .effective_bss_color();
            let state = parse_he20_peer_state(capability, operation)
                .map_err(StaAssociatedPeerError::HePeer)?;
            (capability, Some(capabilities), Some(state), bss_color)
        } else {
            let capability = access_point.he_capability_ie_bytes();
            (
                capability,
                parse_he20_capabilities(capability).ok(),
                None,
                parse_he20_operation(access_point.he_operation_ie_bytes())
                    .map_or(0, |operation| operation.effective_bss_color()),
            )
        };
        // SOURCE(esp32s31): complete pinned `libnet80211.a[wl_cnx.o]::ic_set_sta`
        // +0x140..0x150 passes the peer's A-MPDU Parameters bits 4:2 (its
        // Minimum MPDU Start Spacing, IEEE Std 802.11-2020, 9.4.2.55.3) and
        // the AID to `hal_he_set_mmss_and_aid`.
        let he_association = match he_peer_state {
            Some(state) => Some(
                oer_protocols_ieee80211_contracts_port::He20Association::new(
                    &state,
                    response.association_id,
                    (ht_ampdu_parameters >> 2) & 0x07,
                )
                .map_err(StaAssociatedPeerError::HeAssociation)?,
            ),
            None => None,
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
                oer_protocols_ieee80211_core_upper_mac::rate_control::NegotiatedLegacyRates::from_rate_elements(
                    access_point.supported_rates_bytes(),
                    access_point.extended_supported_rates_bytes(),
                )
                .ok_or(StaAssociatedPeerError::NoLegacyRates)?,
            ht_capabilities: access_point.ht_peer_capabilities(),
            ht_ampdu_parameters,
            he_capabilities,
            he_peer_state,
            he_bss_color,
            he_association,
            protection: BssProtection {
                erp,
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
