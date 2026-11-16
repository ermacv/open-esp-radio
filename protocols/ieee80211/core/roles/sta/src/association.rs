//! What a station asks an access point for, and what it negotiates with it
//! from its scan record before any frame: the format once, then the width.

use oer_protocols_ieee80211_contracts_port::{
    Ieee80211Capabilities, PhyFormat, PhyFormatSet, SupportedChannel, Width,
};
use oer_protocols_ieee80211_core_upper_mac::phy_mode::{NegotiatedPhy, PhyMode};
use oer_protocols_ieee80211_core_upper_mac::protection::{
    BasicRates, BssProtection, HePacketPadding, HeTxopDurationRtsThreshold,
};
use oer_protocols_ieee80211_wire::management::CapabilityInformation;

/// Whether `access_point` admits an HE association: it advertises HT
/// Capabilities, HE Capabilities with HE-MCS 0..9 both ways on one stream
/// and an HE Operation element that parses.
pub fn he_admitted(access_point: &oer_protocols_ieee80211_wire::scan::ScanRecord) -> bool {
    access_point.ht_capability_ie_bytes().is_some()
        && oer_protocols_ieee80211_wire::he::parse_he_capabilities(
            access_point.he_capability_ie_bytes(),
        )
        .is_ok_and(|capability| capability.supports_bidirectional_mcs9())
        && oer_protocols_ieee80211_wire::he::parse_he_operation(
            access_point.he_operation_ie_bytes(),
        )
        .is_ok()
}

/// The power elements an HE Association Request carries: the Power
/// Capability and the HE UL-MU power capability, from the composition's
/// calibrated transmit power.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaHePower {
    pub power_capability: oer_protocols_ieee80211_wire::station::StaPowerCapability,
    pub ul_mu_power: oer_protocols_ieee80211_wire::station::HeUlMuPowerCapability,
}

/// What a station asks an access point for: the formats it admits and the
/// widest width it may use, checked against what the chip `K` offers. A
/// request of constants is built in a const item, where one outside the
/// chip does not compile; one that arrives at run time gets the same answer
/// as a handled error. The power elements an HE request carries are the
/// station's fact ([`StaConfig::he_power`](crate::machine::StaConfig::he_power)).
pub struct StaPhyRequest<K> {
    formats: PhyFormatSet,
    widest: Width,
    brand: core::marker::PhantomData<fn() -> K>,
}

impl<K> Clone for StaPhyRequest<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for StaPhyRequest<K> {}

impl<K> PartialEq for StaPhyRequest<K> {
    fn eq(&self, other: &Self) -> bool {
        self.formats == other.formats && self.widest == other.widest
    }
}

impl<K> Eq for StaPhyRequest<K> {}

impl<K> core::fmt::Debug for StaPhyRequest<K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StaPhyRequest")
            .field("formats", &self.formats)
            .field("widest", &self.widest)
            .finish()
    }
}

/// Why a request is none.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaPhyRequestError {
    /// It admits no format.
    NoFormat,
    /// It admits a format the chip offers at no width.
    FormatNotOffered(PhyFormat),
    /// No format it admits is offered at its widest width.
    WidthNotOffered(Width),
}

impl<K: Ieee80211Capabilities> StaPhyRequest<K> {
    /// A request of `formats` up to `widest`: each format offered by the
    /// chip at some width, and the widest width offered for one of them.
    pub const fn new(formats: PhyFormatSet, widest: Width) -> Result<Self, StaPhyRequestError> {
        let offered = K::CAPS.formats;
        let mut admitted_widest = false;
        let mut any = false;
        let all = [PhyFormat::NonHt, PhyFormat::Ht, PhyFormat::He];
        let mut index = 0;
        while index < all.len() {
            let format = all[index];
            index += 1;
            if !formats.contains(format) {
                continue;
            }
            any = true;
            let widths = offered.of(format);
            if !widths.contains_width(Width::Mhz20) && !widths.contains_width(Width::Mhz40) {
                return Err(StaPhyRequestError::FormatNotOffered(format));
            }
            admitted_widest = admitted_widest || widths.contains_width(widest);
        }
        if !any {
            return Err(StaPhyRequestError::NoFormat);
        }
        if !admitted_widest {
            return Err(StaPhyRequestError::WidthNotOffered(widest));
        }
        Ok(Self {
            formats,
            widest,
            brand: core::marker::PhantomData,
        })
    }
}

impl<K> StaPhyRequest<K> {
    /// The formats it admits.
    pub const fn formats(&self) -> PhyFormatSet {
        self.formats
    }

    /// The widest width it may use.
    pub const fn widest(&self) -> Width {
        self.widest
    }
}

/// The PHY and channel a join of `access_point`, scanned on `scanned`,
/// takes under `request`, decided from its scan record before any frame;
/// `None` when it offers no format the request admits.
///
/// The format first: HE where the request admits it and the access point
/// admits HE ([`he_admitted`]), at 20 MHz on `scanned`, with the station's
/// `he_power`; else HT where both
/// admit it, at 40 MHz on the one of `wide` whose primary is `scanned`'s
/// and whose secondary the access point's HT elements name, when the
/// request's widest width and the widths HT is offered at allow it, else
/// at 20 MHz; else non-HT where the request admits it.
pub fn negotiate<K: Ieee80211Capabilities>(
    request: &StaPhyRequest<K>,
    he_power: StaHePower,
    access_point: &oer_protocols_ieee80211_wire::scan::ScanRecord,
    wide: &[SupportedChannel<K>],
    scanned: SupportedChannel<K>,
) -> Option<(SupportedChannel<K>, NegotiatedPhy)> {
    use oer_protocols_ieee80211_wire::{channel::ChannelWidth, scan::HtSecondaryChannel};
    let formats = request.formats();
    if formats.contains(PhyFormat::He) && he_admitted(access_point) {
        return Some((
            scanned,
            NegotiatedPhy::He {
                power_capability: he_power.power_capability,
                he_ul_mu_power: he_power.ul_mu_power,
            },
        ));
    }
    if formats.contains(PhyFormat::Ht) && access_point.ht_capability_ie_bytes().is_some() {
        let primary = scanned.channel();
        let wide = (request.widest == Width::Mhz40
            && K::CAPS.formats.ht.contains_width(Width::Mhz40))
        .then(|| access_point.secondary_channel(primary.number()))
        .flatten()
        .and_then(|secondary| {
            let width = match secondary {
                HtSecondaryChannel::Above => ChannelWidth::Mhz40Above,
                HtSecondaryChannel::Below => ChannelWidth::Mhz40Below,
            };
            wide.iter().copied().find(|channel| {
                let channel = channel.channel();
                channel.band() == primary.band()
                    && channel.number() == primary.number()
                    && channel.width() == width
            })
        });
        return Some(match wide {
            Some(channel) => (
                channel,
                NegotiatedPhy::Ht {
                    width: Width::Mhz40,
                },
            ),
            None => (
                scanned,
                NegotiatedPhy::Ht {
                    width: Width::Mhz20,
                },
            ),
        });
    }
    formats
        .contains(PhyFormat::NonHt)
        .then_some((scanned, NegotiatedPhy::NonHt))
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
        Err(_) => ErpProtection::new(
            oer_protocols_ieee80211_wire::protection::ErpInformation::from_bytes([0])
                .with_use_protection(true)
                .with_barker(true),
        ),
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
    pub he_capabilities: Option<oer_protocols_ieee80211_wire::he::HeCapabilities>,
    /// The HE state of an HE association.
    pub he_peer_state: Option<oer_protocols_ieee80211_wire::he::HePeerState>,
    /// The BSS color of the access point's HE Operation, zero without one.
    pub he_bss_color: u8,
    /// What the station's MAC follows of an HE association: its peer
    /// state, the AID and the access point's minimum MPDU start spacing.
    pub he_association: Option<oer_protocols_ieee80211_contracts_port::HeAssociation>,
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
    HeAssociation(oer_protocols_ieee80211_contracts_port::HeAssociationError),
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
            he::{parse_he_capabilities, parse_he_operation, parse_he_peer_state},
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
        let (he_capability, he_capabilities, he_peer_state, he_bss_color) =
            if phy.format == oer_protocols_ieee80211_contracts_port::PhyFormat::He {
                let he = &response.he;
                if he.unusable() {
                    return Err(StaAssociatedPeerError::HeElementsUnusable);
                }
                let (capability, operation) = (he.capability(), he.operation());
                if capability.is_empty() || operation.is_empty() {
                    return Err(StaAssociatedPeerError::HeNotGranted);
                }
                let capabilities = parse_he_capabilities(capability)
                    .map_err(StaAssociatedPeerError::HeCapabilities)?;
                if !capabilities.supports_bidirectional_mcs9() {
                    return Err(StaAssociatedPeerError::HeUnsupported);
                }
                let bss_color = parse_he_operation(operation)
                    .map_err(StaAssociatedPeerError::HeOperation)?
                    .effective_bss_color();
                let state = parse_he_peer_state(capability, operation)
                    .map_err(StaAssociatedPeerError::HePeer)?;
                (capability, Some(capabilities), Some(state), bss_color)
            } else {
                let capability = access_point.he_capability_ie_bytes();
                (
                    capability,
                    parse_he_capabilities(capability).ok(),
                    None,
                    parse_he_operation(access_point.he_operation_ie_bytes())
                        .map_or(0, |operation| operation.effective_bss_color()),
                )
            };
        // SOURCE(esp32s31): complete pinned `libnet80211.a[wl_cnx.o]::ic_set_sta`
        // +0x140..0x150 passes the peer's A-MPDU Parameters bits 4:2 (its
        // Minimum MPDU Start Spacing, IEEE Std 802.11-2020, 9.4.2.55.3) and
        // the AID to `hal_he_set_mmss_and_aid`.
        let he_association = match he_peer_state {
            Some(state) => Some(
                oer_protocols_ieee80211_contracts_port::HeAssociation::new(
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
                    .and_then(|state| state.rts_threshold())
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
