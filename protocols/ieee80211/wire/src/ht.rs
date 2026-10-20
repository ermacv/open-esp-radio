//! Shared, allocation-free IEEE 802.11n HT information elements.
//!
//! These records encode caller-selected one-spatial-stream HT capabilities
//! shared by the radio roles. The validated channel geometry decides whether a BSS
//! advertises HT20 or HT40; callers cannot independently select contradictory
//! capability and operation elements.
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

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
    pub const CAPABILITY_IE_MASK: u8 = HtMcs32Octet::from_bytes([0])
        .with_supported(true)
        .to_bytes()[0];

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
        element[Self::CAPABILITY_IE_BYTE] =
            HtMcs32Octet::from_bytes([element[Self::CAPABILITY_IE_BYTE]])
                .with_supported(true)
                .to_bytes()[0];
        element[17] = HtTxMcs::from_bytes([element[17]])
            .with_defined(true)
            .with_unequal(true)
            .to_bytes()[0];
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
    let width40 = !matches!(channel.width(), WifiChannelWidth::Mhz20);
    let [c0, c1] = HtCapabilityInfo::from_bytes(local_ht.capability_info.to_le_bytes())
        .with_width40(width40)
        .with_short_gi20(true)
        .with_short_gi40(width40)
        .to_bytes();
    let mut element = [0_u8; HT_CAPABILITY_IE_LEN];
    element[0] = 45;
    element[1] = 26;
    element[2] = c0;
    element[3] = c1;
    let local_ampdu_parameters = local_ht.ampdu_parameters;
    element[4] = match peer {
        Some(peer) => {
            let local = HtAmpduParameters::from_bytes([local_ampdu_parameters]);
            let peer = HtAmpduParameters::from_bytes([peer.ampdu_parameters]);
            let exponent = if local.exponent() < peer.exponent() {
                local.exponent()
            } else {
                peer.exponent()
            };
            let spacing = if local.spacing() > peer.spacing() {
                local.spacing()
            } else {
                peer.spacing()
            };
            let fields = crate::encoding::validated(
                HtAmpduParameters::from_bytes([0]).with_exponent(exponent),
            );
            crate::encoding::validated(fields.with_spacing(spacing)).to_bytes()[0]
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
        HtCapabilityInfo::from_bytes(self.capability_info.to_le_bytes()).width40()
    }

    /// HT Capabilities Information HT-Greenfield.
    pub const fn supports_greenfield(self) -> bool {
        HtCapabilityInfo::from_bytes(self.capability_info.to_le_bytes()).greenfield()
    }

    pub const fn supports_short_guard_interval(self, width: WifiChannelWidth) -> bool {
        match width {
            WifiChannelWidth::Mhz20 => {
                HtCapabilityInfo::from_bytes(self.capability_info.to_le_bytes()).short_gi20()
            }
            WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below => {
                self.supports_40_mhz()
                    && HtCapabilityInfo::from_bytes(self.capability_info.to_le_bytes()).short_gi40()
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
        #[expect(
            clippy::expect_used,
            reason = "`ht_peer_capabilities`, the only constructor, refuses a zero MCS bitmap"
        )]
        let highest = self
            .rx_mcs_0_to_7
            .checked_ilog2()
            .expect("a peer advertises at least one receive MCS");
        // The highest set bit of an octet is below eight.
        highest as u8
    }
}

/// Parse a complete information-element stream into one-stream HT facts.
pub fn ht_peer_capabilities(bytes: &[u8]) -> Option<HtPeerCapabilities> {
    let mut remaining = oer_wire::Reader::new(bytes);
    while let [_, length, ..] = *remaining.remaining() {
        let record = remaining.take(usize::from(length).checked_add(2)?).ok()?;
        if let Ok(record) = <&[u8; HT_CAPABILITY_IE_LEN]>::try_from(record)
            && let [45, _, c0, c1, ampdu_parameters, rx_mcs_0_to_7, ..] = *record
            && rx_mcs_0_to_7 != 0
        {
            return Some(HtPeerCapabilities {
                capability_info: u16::from_le_bytes([c0, c1]),
                ampdu_parameters,
                rx_mcs_0_to_7,
                rx_ht_duplicate_mcs32: HtMcs32Octet::from_bytes([
                    record[HtDuplicateMcs32::CAPABILITY_IE_BYTE]
                ])
                .supported(),
            });
        }
    }
    None
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.55.2 (HT Capabilities Info field).
    pub struct HtCapabilityInfo[2] (little) {
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.55.2 (HT Capabilities Info field).
        pub width40, with_width40: bool = 1..2;
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.55.2 (HT Capabilities Info field).
        pub greenfield, with_greenfield: bool = 4..5;
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.55.2 (HT Capabilities Info field).
        pub short_gi20, with_short_gi20: bool = 5..6;
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.55.2 (HT Capabilities Info field).
        pub short_gi40, with_short_gi40: bool = 6..7;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.55.3 (A-MPDU Parameters field).
    pub struct HtAmpduParameters[1] (little) {
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.55.3 (A-MPDU Parameters field).
        pub exponent, with_exponent: u8 = 0..2;
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.55.3 (A-MPDU Parameters field).
        pub spacing, with_spacing: u8 = 2..5;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.56 (HT Operation element).
    pub struct HtOperationInfo[1] (little) {
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.56 (HT Operation element).
        pub secondary, with_secondary: u8 = 0..2;
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.56 (HT Operation element).
        pub width40, with_width40: bool = 2..3;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.55.4 (Supported MCS Set field).
    pub struct HtTxMcs[1] (little) {
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.55.4 (Supported MCS Set field).
        pub defined, with_defined: bool = 0..1;
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.55.4 (Supported MCS Set field).
        pub unequal, with_unequal: bool = 1..2;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.55.4 (Supported MCS Set, RX MCS bitmap octet 4).
    pub struct HtMcs32Octet[1] (little) {
        /// SOURCE: IEEE Std 802.11-2020, 9.4.2.55.4 (Supported MCS Set, RX MCS bitmap octet 4).
        pub supported, with_supported: bool = 0..1;
    }
}

#[cfg(test)]
mod tests;
