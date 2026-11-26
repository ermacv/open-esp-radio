//! The station interface's HE association, which the backend's MAC
//! follows ([`ValueCommand::SetHe`](crate::ValueCommand::SetHe)).
//!
//! It is set in two stages, in the order the pinned vendor station sets it:
//! the BSS before authentication ([`StationHe::Bss`],
//! `libnet80211.a[wl_cnx.o]::cnx_connect_to_bss`), the association after
//! the Association Response ([`StationHe::He`],
//! `ieee80211_parse_hecap`, `ieee80211_parse_heopr` and `ic_set_sta`).

use oer_protocols_ieee80211_wire::he::{HeOperation, HePeerState};
use oer_protocols_ieee80211_wire::management::{AidField, AssociationId};

use oer_protocols_ieee80211_wire::he::{BssColor, MaxCoHostedBssidIndicator};
use oer_protocols_ieee80211_wire::ht::MinimumMpduStartSpacing;

/// The HE state of the station's interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationHe {
    /// No HE BSS: the MAC matches no HE BSS, its color bitmap is cleared and
    /// the interface's BSS color is disabled and zero, for its own
    /// transmissions too. A join of a BSS without HE starts with it, as one
    /// HE association ends with it.
    Off,
    /// Before authentication with an HE BSS: HE BSSID matching on, the
    /// color bitmap cleared and the BSS color the scan heard, which the
    /// station's HE transmissions carry.
    Bss(HeBssColor),
    /// After the HE BSS's Association Response: its HE parameters (its BSS
    /// color included, which replaces the scan's for the MAC and the
    /// station's HE transmissions), the AID and the minimum MPDU start
    /// spacing.
    He(HeAssociation),
}

/// Whether an HE BSS's color is in use, as its HE Operation's BSS Color
/// Disabled subfield says (IEEE 802.11ax-2021, 9.4.2.249).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BssColorUse {
    /// The color is disabled: transmissions carry zero.
    Disabled,
    /// The color is in use.
    Enabled,
}

/// Whether an HE BSS's color is partial, as its Partial BSS Color subfield
/// says (IEEE 802.11ax-2021, 9.4.2.249).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BssColorScope {
    /// The whole color.
    Full,
    /// A partial color.
    Partial,
}

/// The BSS color of an HE BSS as its HE Operation names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeBssColor {
    color: BssColor,
    used: BssColorUse,
    scope: BssColorScope,
}

impl HeBssColor {
    /// The color `operation` names.
    pub const fn of(operation: &HeOperation) -> Self {
        Self {
            color: operation.bss_color(),
            used: if operation.bss_color_enabled() {
                BssColorUse::Enabled
            } else {
                BssColorUse::Disabled
            },
            scope: if operation.partial_bss_color() {
                BssColorScope::Partial
            } else {
                BssColorScope::Full
            },
        }
    }

    /// The color, whether it is in use and whether it is partial.
    pub const fn get(&self) -> (BssColor, BssColorUse, BssColorScope) {
        (self.color, self.used, self.scope)
    }

    /// The color the station's HE transmissions carry: the BSS color while
    /// it is enabled, else zero.
    pub const fn transmit_color(&self) -> BssColor {
        if matches!(self.used, BssColorUse::Enabled) {
            self.color
        } else {
            BssColor::ZERO
        }
    }
}

/// What the backend's MAC needs of an HE20 association: the access point's
/// HE parameters from its Association Response, the station's association
/// identifier and the access point's minimum MPDU start spacing. Each value
/// lies within its field's range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeAssociation {
    color: HeBssColor,
    association_id: AssociationId,
    minimum_mpdu_start_spacing: MinimumMpduStartSpacing,
    packet_padding: oer_protocols_ieee80211_wire::he::HePacketPadding,
    default_packet_extension: oer_protocols_ieee80211_wire::he::DefaultPeDuration,
    max_co_hosted_bssid_indicator: Option<MaxCoHostedBssidIndicator>,
    extended_range_single_user_disabled: bool,
}

/// Why an HE20 association's values are none.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeAssociationError {
    /// The AID is outside 1 to 2007 (IEEE Std 802.11-2020, 9.4.1.8).
    AssociationId(AidField),
    /// The Max Co-Hosted BSSID Indicator is above 8: a co-hosted set of
    /// more than 2^8 BSSIDs, whose mask the MAC's eight-bit field cannot
    /// hold.
    MaxCoHostedBssidIndicator(MaxCoHostedBssidIndicator),
}

impl HeAssociation {
    /// The largest Max Co-Hosted BSSID Indicator.
    pub const MAX_CO_HOSTED_BSSID_INDICATOR: u8 = 8;

    /// The association of `peer` with AID `association_id` and the access
    /// point's minimum MPDU start spacing code.
    pub const fn new(
        peer: &HePeerState,
        association_id: AidField,
        minimum_mpdu_start_spacing: MinimumMpduStartSpacing,
    ) -> Result<Self, HeAssociationError> {
        let Some(association_id) = AssociationId::from_field(association_id) else {
            return Err(HeAssociationError::AssociationId(association_id));
        };
        if let Some(indicator) = peer.max_co_hosted_bssid_indicator
            && indicator.get() > Self::MAX_CO_HOSTED_BSSID_INDICATOR
        {
            return Err(HeAssociationError::MaxCoHostedBssidIndicator(indicator));
        }
        let color = peer.bss_color();
        Ok(Self {
            color: HeBssColor {
                color,
                used: if peer.bss_color_enabled() {
                    BssColorUse::Enabled
                } else {
                    BssColorUse::Disabled
                },
                scope: if peer.partial_bss_color() {
                    BssColorScope::Partial
                } else {
                    BssColorScope::Full
                },
            },
            association_id,
            minimum_mpdu_start_spacing,
            packet_padding: peer.packet_padding,
            default_packet_extension: peer.default_packet_extension_duration(),
            max_co_hosted_bssid_indicator: peer.max_co_hosted_bssid_indicator,
            extended_range_single_user_disabled: peer.extended_range_single_user_disabled(),
        })
    }

    /// The BSS color of the Association Response's HE Operation.
    pub const fn color(&self) -> HeBssColor {
        self.color
    }

    /// The station's AID.
    pub const fn association_id(&self) -> AssociationId {
        self.association_id
    }

    /// The access point's minimum MPDU start spacing.
    pub const fn minimum_mpdu_start_spacing(&self) -> MinimumMpduStartSpacing {
        self.minimum_mpdu_start_spacing
    }

    /// The access point's packet padding.
    pub const fn packet_padding(&self) -> oer_protocols_ieee80211_wire::he::HePacketPadding {
        self.packet_padding
    }

    /// The HE Operation's default PE duration.
    pub const fn default_packet_extension(
        &self,
    ) -> oer_protocols_ieee80211_wire::he::DefaultPeDuration {
        self.default_packet_extension
    }

    /// The HE Operation's Max Co-Hosted BSSID Indicator, 0 to 8, when it
    /// announces a co-hosted BSS.
    pub const fn max_co_hosted_bssid_indicator(&self) -> Option<MaxCoHostedBssidIndicator> {
        self.max_co_hosted_bssid_indicator
    }

    /// Whether the HE Operation disables ER SU PPDUs.
    pub const fn extended_range_single_user_disabled(&self) -> bool {
        self.extended_range_single_user_disabled
    }
}

#[cfg(test)]
mod tests;
