//! Which user priority an outgoing Ethernet frame requests: its outer VLAN
//! Priority Code Point, else its IP DSCP through the station-safe mapping of
//! RFC 8325. The station decides the final priority from its link.
//!
//! The mapping is a policy, not an authorization: an application's marking
//! can request Voice (EF, VOICE-ADMIT or PCP 6/7). An explicit outer PCP
//! takes precedence over an encapsulated DSCP. The classifier never invents
//! priority from transport ports or payload bytes.

use oer_ieee80211_wire::qos::{WmmAccessCategory, WmmUserPriority};
use oer_network_wire::ethernet::EtherType;
use oer_network_wire::ip::{Dscp, Ipv4Header, Ipv6Header};
use oer_network_wire::vlan::VlanTag;

/// Widely deployed default mapping based on the three DSCP MSBs.
pub const fn default_user_priority(dscp: Dscp) -> WmmUserPriority {
    user_priority_of_code(dscp.class_selector())
}

/// RFC 8325 station-safe DSCP mapping.
///
/// Only the standardized service-class codepoints receive elevated
/// priority. Unknown codepoints and endpoint-sourced CS6/CS7 are bleached
/// to UP0, so a marking does not acquire the network-control priority.
/// RFC 8622's Lower Effort codepoint joins CS1 in the background category.
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
    /// A valid frame without a VLAN tag or an IP header.
    #[default]
    Unmarked,
    /// A frame whose VLAN or IP header the codec refused.
    Malformed,
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

    const fn best_effort(source: WmmClassificationSource) -> Self {
        Self::from_user_priority(WmmUserPriority::UP0, source)
    }
}

/// Classify an Ethernet frame, given as its EtherType and the payload after
/// its header, from an outer 802.1D PCP or an IP DSCP.
///
/// A valid frame of another type is `Unmarked`, one the codec refuses is
/// `Malformed`; both request best effort.
pub fn classify_wmm(ether_type: EtherType, payload: &[u8]) -> WmmTrafficClass {
    use WmmClassificationSource::{Malformed, Unmarked};
    if ether_type.is_vlan_tag() {
        let Ok((tag, _)) = VlanTag::parse(payload) else {
            return WmmTrafficClass::best_effort(Malformed);
        };
        let user_priority = user_priority_of_code(tag.tci.pcp());
        // The explicit outer PCP is authoritative even when it is zero.
        return WmmTrafficClass::from_user_priority(
            user_priority,
            WmmClassificationSource::Ieee8021d(user_priority),
        );
    }
    let dscp = match ether_type {
        EtherType::IPV4 => match Ipv4Header::parse(payload) {
            Ok(ip) => ip.dscp(),
            Err(_) => return WmmTrafficClass::best_effort(Malformed),
        },
        EtherType::IPV6 => match Ipv6Header::parse(payload) {
            Ok(ip) => ip.dscp(),
            Err(_) => return WmmTrafficClass::best_effort(Malformed),
        },
        _ => return WmmTrafficClass::best_effort(Unmarked),
    };
    WmmTrafficClass::from_user_priority(
        station_user_priority(dscp),
        WmmClassificationSource::Dscp(dscp),
    )
}

/// The user priority of a three-bit priority code; a wider value selects
/// UP7.
const fn user_priority_of_code(code: u8) -> WmmUserPriority {
    match WmmUserPriority::new(code) {
        Some(priority) => priority,
        None => WmmUserPriority::UP7,
    }
}

#[cfg(test)]
mod tests;
