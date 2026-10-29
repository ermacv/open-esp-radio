//! Traffic classification of the MAC data service: which user priority and
//! access category an outgoing network frame takes, and how admission
//! control downgrades an access category. The priority and category values
//! and the header fields read here are the formats of `oer_ieee80211_wire`.

use oer_ieee80211_wire::{
    ethertype::{ETHER_TYPE_IPV4, ETHER_TYPE_IPV6, ETHER_TYPE_PROVIDER_VLAN, ETHER_TYPE_VLAN},
    qos::{
        DsField, Dscp, Ipv4VersionIhl, Ipv6HeaderPrefix, VlanTci, WmmAccessCategory,
        WmmUserPriority,
    },
};
use oer_network::ETHERNET_HEADER_LEN;

/// Next lower access category used when the requested AC requires an
/// admission which the station does not own.
pub const fn downgrade(category: WmmAccessCategory) -> Option<WmmAccessCategory> {
    match category {
        WmmAccessCategory::Voice => Some(WmmAccessCategory::Video),
        WmmAccessCategory::Video => Some(WmmAccessCategory::BestEffort),
        WmmAccessCategory::BestEffort => Some(WmmAccessCategory::Background),
        WmmAccessCategory::Background => None,
    }
}

/// Canonical user priority used when ACM policy downgrades into `category`.
pub const fn canonical_user_priority(category: WmmAccessCategory) -> WmmUserPriority {
    match category {
        WmmAccessCategory::Voice => WmmUserPriority::UP7,
        // The recovered station classifier enters VI at UP5.
        WmmAccessCategory::Video => WmmUserPriority::UP5,
        WmmAccessCategory::BestEffort => WmmUserPriority::UP0,
        WmmAccessCategory::Background => WmmUserPriority::UP1,
    }
}

/// The user priority of a three-bit priority code; a wider value selects
/// UP7.
const fn user_priority_of_code(code: u8) -> WmmUserPriority {
    match WmmUserPriority::new(code) {
        Some(priority) => priority,
        None => WmmUserPriority::UP7,
    }
}

/// Widely deployed default mapping based on the three DSCP MSBs.
pub const fn default_user_priority(dscp: Dscp) -> WmmUserPriority {
    user_priority_of_code(dscp.class_selector())
}

/// RFC 8325 station-safe DSCP mapping.
///
/// Only the standardized service-class codepoints receive elevated
/// priority. Unknown codepoints and endpoint-sourced CS6/CS7 are bleached
/// to UP0, preventing an application marking from acquiring the reserved
/// network-control priority. RFC 8622's Lower Effort codepoint joins CS1
/// in the background category.
pub const fn station_user_priority(dscp: Dscp) -> WmmUserPriority {
    match dscp.value() {
        // Lower Effort and CS1.
        1 | 8 => WmmUserPriority::UP1,
        // AF2x low-latency data.
        18 | 20 | 22 => WmmUserPriority::UP3,
        // AF3x, CS3, AF4x and CS4 video classes.
        24 | 26 | 28 | 30 | 32 | 34 | 36 | 38 => WmmUserPriority::UP4,
        // CS5 signaling.
        40 => WmmUserPriority::UP5,
        // VOICE-ADMIT and EF telephony.
        44 | 46 => WmmUserPriority::UP6,
        // DF, OAM, AF1x, CS6/CS7 and unassigned values fail closed to BE.
        _ => WmmUserPriority::UP0,
    }
}

/// Header field which selected one network frame's WMM class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WmmClassificationSource {
    /// Untagged non-IP traffic or a truncated network header.
    #[default]
    Unmarked,
    /// The outer IEEE 802.1Q/802.1ad priority-code-point field.
    Ieee8021d(WmmUserPriority),
    /// IPv4 or IPv6 DSCP mapped with the station-safe policy.
    Dscp(Dscp),
}

/// Complete portable WMM classification for one network frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WmmTrafficClass {
    pub user_priority: WmmUserPriority,
    pub access_category: WmmAccessCategory,
    pub source: WmmClassificationSource,
}

impl WmmTrafficClass {
    pub const BEST_EFFORT: Self = Self {
        user_priority: WmmUserPriority::UP0,
        access_category: WmmAccessCategory::BestEffort,
        source: WmmClassificationSource::Unmarked,
    };

    pub const fn from_user_priority(
        user_priority: WmmUserPriority,
        source: WmmClassificationSource,
    ) -> Self {
        Self {
            user_priority,
            access_category: user_priority.access_category(),
            source,
        }
    }
}

/// Classify an Ethernet frame from an outer 802.1D PCP or an IP DSCP.
///
/// An explicit VLAN PCP takes precedence over an encapsulated DSCP. Untagged
/// non-IP and truncated headers remain best effort. The function never
/// invents priority from transport ports or payload bytes.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
pub fn classify_ethernet_wmm(ethernet: &[u8]) -> WmmTrafficClass {
    let Some((&[.., ether_type_high, ether_type_low], payload)) =
        ethernet.split_first_chunk::<ETHERNET_HEADER_LEN>()
    else {
        return WmmTrafficClass::BEST_EFFORT;
    };

    let ether_type = u16::from_be_bytes([ether_type_high, ether_type_low]);
    if matches!(ether_type, ETHER_TYPE_VLAN | ETHER_TYPE_PROVIDER_VLAN) {
        let Some(&[tci_high, tci_low, _, _]) = payload.first_chunk::<4>() else {
            return WmmTrafficClass::BEST_EFFORT;
        };
        let tci = VlanTci::from_bytes([tci_high, tci_low]);
        let user_priority = user_priority_of_code(tci.pcp());
        // The explicit outer PCP is authoritative even when it is zero.
        return WmmTrafficClass::from_user_priority(
            user_priority,
            WmmClassificationSource::Ieee8021d(user_priority),
        );
    }

    let dscp = match ether_type {
        ETHER_TYPE_IPV4 => {
            let Some(&[version_and_length, services]) = payload.first_chunk::<2>() else {
                return WmmTrafficClass::BEST_EFFORT;
            };
            if Ipv4VersionIhl::from_bytes([version_and_length]).version() != 4 {
                return WmmTrafficClass::BEST_EFFORT;
            }
            Dscp::from_field(DsField::from_bytes([services]).dscp())
        }
        ETHER_TYPE_IPV6 => {
            let Some(&[version_and_class, class_and_label]) = payload.first_chunk::<2>() else {
                return WmmTrafficClass::BEST_EFFORT;
            };
            let prefix = Ipv6HeaderPrefix::from_bytes([version_and_class, class_and_label]);
            if prefix.version() != 6 {
                return WmmTrafficClass::BEST_EFFORT;
            }
            Dscp::from_field(DsField::from_bytes([prefix.traffic_class()]).dscp())
        }
        _ => return WmmTrafficClass::BEST_EFFORT,
    };
    let user_priority = station_user_priority(dscp);
    WmmTrafficClass::from_user_priority(user_priority, WmmClassificationSource::Dscp(dscp))
}

#[cfg(test)]
mod tests;
