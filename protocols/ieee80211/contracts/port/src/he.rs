//! The station interface's HE association, which the backend's MAC
//! follows ([`PortCommand::SetHe`](crate::PortCommand::SetHe)).
//!
//! It is set in two stages, in the order the pinned vendor station sets it:
//! the BSS before authentication ([`StationHe::Bss`],
//! `libnet80211.a[wl_cnx.o]::cnx_connect_to_bss`), the association after
//! the Association Response ([`StationHe::He20`],
//! `ieee80211_parse_hecap`, `ieee80211_parse_heopr` and `ic_set_sta`).

use oer_protocols_ieee80211_wire::he::{He20Operation, He20PeerState};

use crate::BssColor;

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
    /// After the HE BSS's Association Response: its HE parameters, the AID
    /// and the minimum MPDU start spacing.
    He20(He20Association),
}

/// The BSS color of an HE BSS as its HE Operation names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeBssColor {
    color: BssColor,
    enabled: bool,
    partial: bool,
}

impl HeBssColor {
    /// The color `operation` names.
    pub const fn of(operation: &He20Operation) -> Option<Self> {
        match BssColor::new(operation.bss_color) {
            Some(color) => Some(Self {
                color,
                enabled: operation.bss_color_enabled,
                partial: operation.partial_bss_color,
            }),
            None => None,
        }
    }

    /// The color, whether it is enabled and whether it is partial.
    pub const fn get(&self) -> (BssColor, bool, bool) {
        (self.color, self.enabled, self.partial)
    }

    /// The color the station's HE transmissions carry: the BSS color while
    /// it is enabled, else zero.
    pub const fn transmit_color(&self) -> BssColor {
        if self.enabled {
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
pub struct He20Association {
    association_id: u16,
    minimum_mpdu_start_spacing: u8,
    packet_padding_eight_us: u8,
    default_packet_extension: u8,
    max_co_hosted_bssid_indicator: Option<u8>,
    extended_range_single_user_disabled: bool,
}

/// Why an HE20 association's values are none.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum He20AssociationError {
    /// The AID is outside 1 to 2007 (IEEE Std 802.11-2020, 9.4.1.8).
    AssociationId(u16),
    /// The minimum MPDU start spacing code is above 7 (IEEE Std
    /// 802.11-2020, 9.4.2.55.3).
    MinimumMpduStartSpacing(u8),
    /// The nominal packet padding is above 16 us (code 2, IEEE Std
    /// 802.11ax-2021, 9.4.2.248.3, clause number as Linux cites it).
    PacketPadding(u8),
    /// The default PE duration code is above 7 (IEEE Std 802.11ax-2021,
    /// 9.4.2.249, clause number as Linux cites it).
    DefaultPacketExtension(u8),
    /// The Max Co-Hosted BSSID Indicator is above 8: a co-hosted set of
    /// more than 2^8 BSSIDs, whose mask the MAC's eight-bit field cannot
    /// hold.
    MaxCoHostedBssidIndicator(u8),
}

impl He20Association {
    /// The largest AID (IEEE Std 802.11-2020, 9.4.1.8).
    pub const MAX_ASSOCIATION_ID: u16 = 2007;
    /// The largest minimum MPDU start spacing code.
    pub const MAX_MINIMUM_MPDU_START_SPACING: u8 = 7;
    /// The largest nominal packet padding, in 8 us units.
    pub const MAX_PACKET_PADDING_EIGHT_US: u8 = 2;
    /// The largest default PE duration code.
    pub const MAX_DEFAULT_PACKET_EXTENSION: u8 = 7;
    /// The largest Max Co-Hosted BSSID Indicator.
    pub const MAX_CO_HOSTED_BSSID_INDICATOR: u8 = 8;

    /// The association of `peer` with AID `association_id` and the access
    /// point's minimum MPDU start spacing code.
    pub const fn new(
        peer: &He20PeerState,
        association_id: u16,
        minimum_mpdu_start_spacing: u8,
    ) -> Result<Self, He20AssociationError> {
        if association_id == 0 || association_id > Self::MAX_ASSOCIATION_ID {
            return Err(He20AssociationError::AssociationId(association_id));
        }
        if minimum_mpdu_start_spacing > Self::MAX_MINIMUM_MPDU_START_SPACING {
            return Err(He20AssociationError::MinimumMpduStartSpacing(
                minimum_mpdu_start_spacing,
            ));
        }
        if peer.packet_padding_eight_us > Self::MAX_PACKET_PADDING_EIGHT_US {
            return Err(He20AssociationError::PacketPadding(
                peer.packet_padding_eight_us,
            ));
        }
        if peer.default_packet_extension_duration > Self::MAX_DEFAULT_PACKET_EXTENSION {
            return Err(He20AssociationError::DefaultPacketExtension(
                peer.default_packet_extension_duration,
            ));
        }
        if let Some(indicator) = peer.max_co_hosted_bssid_indicator
            && indicator > Self::MAX_CO_HOSTED_BSSID_INDICATOR
        {
            return Err(He20AssociationError::MaxCoHostedBssidIndicator(indicator));
        }
        Ok(Self {
            association_id,
            minimum_mpdu_start_spacing,
            packet_padding_eight_us: peer.packet_padding_eight_us,
            default_packet_extension: peer.default_packet_extension_duration,
            max_co_hosted_bssid_indicator: peer.max_co_hosted_bssid_indicator,
            extended_range_single_user_disabled: peer.extended_range_single_user_disabled,
        })
    }

    /// The station's AID, 1 to 2007.
    pub const fn association_id(&self) -> u16 {
        self.association_id
    }

    /// The access point's minimum MPDU start spacing code, 0 to 7.
    pub const fn minimum_mpdu_start_spacing(&self) -> u8 {
        self.minimum_mpdu_start_spacing
    }

    /// The access point's nominal packet padding in 8 us units, 0 to 2.
    pub const fn packet_padding_eight_us(&self) -> u8 {
        self.packet_padding_eight_us
    }

    /// The HE Operation's default PE duration code, 0 to 7.
    pub const fn default_packet_extension(&self) -> u8 {
        self.default_packet_extension
    }

    /// The HE Operation's Max Co-Hosted BSSID Indicator, 0 to 8, when it
    /// announces a co-hosted BSS.
    pub const fn max_co_hosted_bssid_indicator(&self) -> Option<u8> {
        self.max_co_hosted_bssid_indicator
    }

    /// Whether the HE Operation disables ER SU PPDUs.
    pub const fn extended_range_single_user_disabled(&self) -> bool {
        self.extended_range_single_user_disabled
    }
}

#[cfg(test)]
mod tests;
