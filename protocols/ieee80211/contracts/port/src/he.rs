//! The station interface's HE association, which the backend's MAC
//! follows ([`PortCommand::SetHe`](crate::PortCommand::SetHe)).

use oer_protocols_ieee80211_wire::he::He20PeerState;

use crate::BssColor;

/// The HE state of the station's interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationHe {
    /// No HE association: the MAC matches no HE BSS and the interface's
    /// BSS color is cleared, for its own transmissions too.
    Off,
    /// An HE20 association with the access point the station associated
    /// with.
    He20(He20Association),
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
    color: BssColor,
    color_enabled: bool,
    partial_color: bool,
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
    /// The BSS color is above 63.
    BssColor(u8),
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
        let Some(color) = BssColor::new(peer.bss_color) else {
            return Err(He20AssociationError::BssColor(peer.bss_color));
        };
        Ok(Self {
            association_id,
            minimum_mpdu_start_spacing,
            packet_padding_eight_us: peer.packet_padding_eight_us,
            default_packet_extension: peer.default_packet_extension_duration,
            color,
            color_enabled: peer.bss_color_enabled,
            partial_color: peer.partial_bss_color,
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

    /// The HE Operation's BSS color, whether it is enabled, and whether it
    /// is partial.
    pub const fn color(&self) -> (BssColor, bool, bool) {
        (self.color, self.color_enabled, self.partial_color)
    }

    /// The color the station's HE transmissions carry: the BSS color while
    /// it is enabled, else zero.
    pub const fn transmit_color(&self) -> BssColor {
        if self.color_enabled {
            self.color
        } else {
            BssColor::ZERO
        }
    }

    /// Whether the HE Operation disables ER SU PPDUs.
    pub const fn extended_range_single_user_disabled(&self) -> bool {
        self.extended_range_single_user_disabled
    }
}

#[cfg(test)]
mod tests;
