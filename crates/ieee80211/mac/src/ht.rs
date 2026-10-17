//! Shared, allocation-free IEEE 802.11n HT information elements.
//!
//! These records encode caller-selected one-spatial-stream HT capabilities
//! shared by the radio roles. The validated channel geometry decides whether a BSS
//! advertises HT20 or HT40; callers cannot independently select contradictory
//! capability and operation elements.

use crate::channel::{Channel, WifiChannelWidth};
use crate::protection::HtOperationProtection;

pub const HT_CAPABILITY_IE_LEN: usize = 28;
pub const HT_OPERATION_IE_LEN: usize = 24;

/// The standard HT Duplicate modulation-and-coding selector, MCS32.
///
/// MCS32 is a special single-stream 40-MHz duplicate mode. It is not the next
/// member of the ordinary one-spatial-stream MCS0..MCS7 range, so keeping it
/// as a zero-sized marker prevents rate-selection code from ordering it as a
/// faster spatial-stream rate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HtDuplicateMcs32;

impl HtDuplicateMcs32 {
    pub const INDEX: u8 = 32;
    pub const CAPABILITY_IE_BYTE: usize = 9;
    pub const CAPABILITY_IE_MASK: u8 = 1;

    pub const fn new() -> Self {
        Self
    }

    pub const fn supports_width(width: WifiChannelWidth) -> bool {
        matches!(
            width,
            WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below
        )
    }

    /// Advertise receive-only MCS32 in a complete HT Capabilities IE.
    ///
    /// TX MCS Parameters bit one marks the TX and RX sets as unequal. Without
    /// it, setting the RX MCS32 bit would also claim unimplemented TX support.
    pub const fn advertise_receive_only(self, element: &mut [u8; HT_CAPABILITY_IE_LEN]) {
        element[Self::CAPABILITY_IE_BYTE] |= Self::CAPABILITY_IE_MASK;
        element[17] |= 0x03;
    }
}

/// Caller-selected local HT advertisement, independent of peer observations.
///
/// The encoder replaces channel-width and short-GI bits in the capability
/// base with values derived from the validated BSS channel. The caller supplies its
/// reviewed A-MPDU receive limits and one-stream MCS advertisement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtLocalCapabilities {
    capability_info: u16,
    ampdu_parameters: u8,
    rx_mcs_0_to_7: u8,
    tx_mcs_parameters: u8,
}

impl HtLocalCapabilities {
    pub const fn new(
        capability_info: u16,
        ampdu_parameters: u8,
        rx_mcs_0_to_7: u8,
        tx_mcs_parameters: u8,
    ) -> Self {
        Self {
            capability_info,
            ampdu_parameters,
            rx_mcs_0_to_7,
            tx_mcs_parameters,
        }
    }
}

/// Build the complete one-stream HT Capabilities element for one BSS.
pub const fn ht_capability_ie(
    local_ht: HtLocalCapabilities,
    channel: Channel,
) -> [u8; HT_CAPABILITY_IE_LEN] {
    ht_capability_ie_for_peer(local_ht, channel, None)
}

/// Build the HT Capabilities element advertised to one known peer.
///
/// Complete vendor `ieee80211_add_htcap_body` negotiates the receive A-MPDU
/// parameters in an association response: it selects the smaller maximum
/// A-MPDU exponent and the larger minimum MPDU spacing. Beacons have no peer
/// record and therefore retain the local interface value.
pub const fn ht_capability_ie_for_peer(
    local_ht: HtLocalCapabilities,
    channel: Channel,
    peer: Option<HtPeerCapabilities>,
) -> [u8; HT_CAPABILITY_IE_LEN] {
    // Channel geometry owns the width and short-GI bits even when the
    // caller supplies them in the capability base.
    let capability_info: u16 = (local_ht.capability_info & !((1 << 1) | (1 << 5) | (1 << 6)))
        | match channel.width() {
            // Short GI for 20 MHz.
            WifiChannelWidth::Mhz20 => 1 << 5,
            // Supported Channel Width plus short GI for both 20 and 40 MHz.
            WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below => {
                (1 << 1) | (1 << 5) | (1 << 6)
            }
        };
    let mut element = [0_u8; HT_CAPABILITY_IE_LEN];
    element[0] = 45;
    element[1] = 26;
    element[2] = capability_info as u8;
    element[3] = (capability_info >> 8) as u8;
    let local_ampdu_parameters = local_ht.ampdu_parameters;
    element[4] = match peer {
        Some(peer) => {
            let exponent = if local_ampdu_parameters & 0x03 < peer.ampdu_parameters & 0x03 {
                local_ampdu_parameters & 0x03
            } else {
                peer.ampdu_parameters & 0x03
            };
            let local_spacing = local_ampdu_parameters & 0x1c;
            let peer_spacing = peer.ampdu_parameters & 0x1c;
            let spacing = if local_spacing > peer_spacing {
                local_spacing
            } else {
                peer_spacing
            };
            exponent | spacing
        }
        None => local_ampdu_parameters,
    };
    element[5] = local_ht.rx_mcs_0_to_7;
    element[17] = local_ht.tx_mcs_parameters;
    element
}

/// Build the complete HT Operation element for one validated BSS channel and
/// its current protection requirements.
pub const fn ht_operation_ie(
    channel: Channel,
    protection: HtOperationProtection,
) -> [u8; HT_OPERATION_IE_LEN] {
    let mut element = [0_u8; 24];
    element[0] = 61;
    element[1] = 22;
    element[2] = channel.number();
    element[3] = match channel.width() {
        // Secondary-channel offset zero and STA channel width zero select
        // HT20.
        WifiChannelWidth::Mhz20 => 0,
        // IEEE secondary offset one and STA channel width one.
        WifiChannelWidth::Mhz40Above => 0x05,
        // IEEE secondary offset three and STA channel width one.
        WifiChannelWidth::Mhz40Below => 0x07,
    };
    element[4] = protection.information_byte();
    element
}

/// One peer's directly observed one-stream HT receive capabilities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtPeerCapabilities {
    capability_info: u16,
    ampdu_parameters: u8,
    rx_mcs_0_to_7: u8,
    rx_ht_duplicate_mcs32: bool,
}

impl HtPeerCapabilities {
    pub const fn supports_40_mhz(self) -> bool {
        self.capability_info & (1 << 1) != 0
    }

    /// HT Capabilities Information HT-Greenfield.
    pub const fn supports_greenfield(self) -> bool {
        self.capability_info & (1 << 4) != 0
    }

    pub const fn supports_short_guard_interval(self, width: WifiChannelWidth) -> bool {
        match width {
            WifiChannelWidth::Mhz20 => self.capability_info & (1 << 5) != 0,
            WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below => {
                self.supports_40_mhz() && self.capability_info & (1 << 6) != 0
            }
        }
    }

    pub const fn ampdu_parameters(self) -> u8 {
        self.ampdu_parameters
    }

    /// Whether the peer can receive the special 40-MHz HT Duplicate mode.
    ///
    /// A malformed peer which sets MCS32 without also admitting 40 MHz is
    /// rejected here even though its raw Supported MCS Set bit is retained by
    /// the parser.
    pub const fn supports_ht_duplicate_mcs32(self) -> bool {
        self.rx_ht_duplicate_mcs32 && self.supports_40_mhz()
    }

    pub const fn ht_duplicate_mcs32(self) -> Option<HtDuplicateMcs32> {
        if self.supports_ht_duplicate_mcs32() {
            Some(HtDuplicateMcs32::new())
        } else {
            None
        }
    }

    /// Highest common one-spatial-stream MCS advertised by the peer.
    pub const fn highest_rx_mcs(self) -> u8 {
        7 - self.rx_mcs_0_to_7.leading_zeros() as u8
    }
}

/// Parse a complete information-element stream into one-stream HT facts.
pub fn ht_peer_capabilities(bytes: &[u8]) -> Option<HtPeerCapabilities> {
    let mut remaining = bytes;
    while remaining.len() >= 2 {
        let id = remaining[0];
        let length = usize::from(remaining[1]);
        let record = remaining.get(..length.saturating_add(2))?;
        if id == 45 && length == 26 && record[5] != 0 {
            return Some(HtPeerCapabilities {
                capability_info: u16::from_le_bytes([record[2], record[3]]),
                ampdu_parameters: record[4],
                rx_mcs_0_to_7: record[5],
                rx_ht_duplicate_mcs32: record[HtDuplicateMcs32::CAPABILITY_IE_BYTE]
                    & HtDuplicateMcs32::CAPABILITY_IE_MASK
                    != 0,
            });
        }
        remaining = &remaining[record.len()..];
    }
    None
}

#[cfg(test)]
mod tests;
