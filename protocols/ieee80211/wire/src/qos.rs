//! Portable IEEE 802.11 priority values and Ethernet traffic classification.
//!
//! UP-to-AC and DSCP mappings describe traffic intent. The WMM vendor element
//! parser lives in `extensions::wmm`; the transmitter still owns admission and
//! queue selection using these values.
//! Checked priorities and categories select cells of exact eight- and
//! four-cell arrays. The selector retains the bounded value and needs no
//! numeric indexing or repeat validation in a caller.

/// Standard WMM access-category identifier carried in ACI bits 5..6.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum WmmAccessCategory {
    #[default]
    BestEffort = 0,
    Background = 1,
    Video = 2,
    Voice = 3,
}

impl WmmAccessCategory {
    /// Every category, in the order of its ACI (`ALL[c as usize] == c`).
    pub const ALL: [Self; 4] = [Self::BestEffort, Self::Background, Self::Video, Self::Voice];

    pub const fn from_aci(aci: u8) -> Option<Self> {
        match aci {
            0 => Some(Self::BestEffort),
            1 => Some(Self::Background),
            2 => Some(Self::Video),
            3 => Some(Self::Voice),
            _ => None,
        }
    }

    /// Select a cell of the exact four-category array without a numeric index.
    pub const fn select<T>(self, entries: &[T; 4]) -> &T {
        let [be, bk, vi, vo] = entries;
        match self {
            Self::BestEffort => be,
            Self::Background => bk,
            Self::Video => vi,
            Self::Voice => vo,
        }
    }
    pub const fn select_mut<T>(self, entries: &mut [T; 4]) -> &mut T {
        let [be, bk, vi, vo] = entries;
        match self {
            Self::BestEffort => be,
            Self::Background => bk,
            Self::Video => vi,
            Self::Voice => vo,
        }
    }

    /// Next lower access category used when the requested AC requires an
    /// admission which the station does not own.
    pub const fn downgrade(self) -> Option<Self> {
        match self {
            Self::Voice => Some(Self::Video),
            Self::Video => Some(Self::BestEffort),
            Self::BestEffort => Some(Self::Background),
            Self::Background => None,
        }
    }

    /// Canonical user priority used when ACM policy downgrades into this AC.
    pub const fn canonical_user_priority(self) -> WmmUserPriority {
        match self {
            Self::Voice => WmmUserPriority::UP7,
            // The recovered station classifier enters VI at UP5.
            Self::Video => WmmUserPriority::UP5,
            Self::BestEffort => WmmUserPriority::UP0,
            Self::Background => WmmUserPriority::UP1,
        }
    }
}

/// Valid IEEE 802.1D / IEEE 802.11 user priority and QoS TID.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct WmmUserPriority(UserPriority);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
enum UserPriority {
    #[default]
    Up0 = 0,
    Up1 = 1,
    Up2 = 2,
    Up3 = 3,
    Up4 = 4,
    Up5 = 5,
    Up6 = 6,
    Up7 = 7,
}

impl WmmUserPriority {
    pub const UP0: Self = Self(UserPriority::Up0);
    pub const UP1: Self = Self(UserPriority::Up1);
    pub const UP2: Self = Self(UserPriority::Up2);
    pub const UP3: Self = Self(UserPriority::Up3);
    pub const UP4: Self = Self(UserPriority::Up4);
    pub const UP5: Self = Self(UserPriority::Up5);
    pub const UP6: Self = Self(UserPriority::Up6);
    pub const UP7: Self = Self(UserPriority::Up7);
    /// Every priority, in TID order.
    pub const ALL: [Self; 8] = [
        Self::UP0,
        Self::UP1,
        Self::UP2,
        Self::UP3,
        Self::UP4,
        Self::UP5,
        Self::UP6,
        Self::UP7,
    ];
    pub const fn new(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self(UserPriority::Up0)),
            1 => Some(Self(UserPriority::Up1)),
            2 => Some(Self(UserPriority::Up2)),
            3 => Some(Self(UserPriority::Up3)),
            4 => Some(Self(UserPriority::Up4)),
            5 => Some(Self(UserPriority::Up5)),
            6 => Some(Self(UserPriority::Up6)),
            7 => Some(Self(UserPriority::Up7)),
            _ => None,
        }
    }
    pub const fn value(self) -> u8 {
        self.0 as u8
    }
    /// Select a cell of the exact eight-entry array by the closed identifier.
    pub const fn select<T>(self, entries: &[T; 8]) -> &T {
        let [a, b, c, d, e, f, g, h] = entries;
        match self.0 {
            UserPriority::Up0 => a,
            UserPriority::Up1 => b,
            UserPriority::Up2 => c,
            UserPriority::Up3 => d,
            UserPriority::Up4 => e,
            UserPriority::Up5 => f,
            UserPriority::Up6 => g,
            UserPriority::Up7 => h,
        }
    }
    /// Select a cell of the exact eight-entry array by the closed identifier.
    pub const fn select_mut<T>(self, entries: &mut [T; 8]) -> &mut T {
        let [a, b, c, d, e, f, g, h] = entries;
        match self.0 {
            UserPriority::Up0 => a,
            UserPriority::Up1 => b,
            UserPriority::Up2 => c,
            UserPriority::Up3 => d,
            UserPriority::Up4 => e,
            UserPriority::Up5 => f,
            UserPriority::Up6 => g,
            UserPriority::Up7 => h,
        }
    }
    pub const fn access_category(self) -> WmmAccessCategory {
        match self.0 {
            UserPriority::Up0 | UserPriority::Up3 => WmmAccessCategory::BestEffort,
            UserPriority::Up1 | UserPriority::Up2 => WmmAccessCategory::Background,
            UserPriority::Up4 | UserPriority::Up5 => WmmAccessCategory::Video,
            UserPriority::Up6 | UserPriority::Up7 => WmmAccessCategory::Voice,
        }
    }
    const fn from_code(value: u8) -> Self {
        match value {
            0 => Self::UP0,
            1 => Self::UP1,
            2 => Self::UP2,
            3 => Self::UP3,
            4 => Self::UP4,
            5 => Self::UP5,
            6 => Self::UP6,
            _ => Self::UP7,
        }
    }
}

/// Valid six-bit IPv4/IPv6 Differentiated Services Code Point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dscp(oer_wire::U8<6>);

impl Dscp {
    pub const fn new(value: u8) -> Option<Self> {
        match oer_wire::U8::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn value(self) -> u8 {
        self.0.get()
    }

    /// Widely deployed default mapping based on the three DSCP MSBs.
    pub const fn default_user_priority(self) -> WmmUserPriority {
        let field = DsField::from_bytes([0]).with_dscp(self.0);
        WmmUserPriority::from_code(field.class_selector())
    }

    /// RFC 8325 station-safe DSCP mapping.
    ///
    /// Only the standardized service-class codepoints receive elevated
    /// priority. Unknown codepoints and endpoint-sourced CS6/CS7 are bleached
    /// to UP0, preventing an application marking from acquiring the reserved
    /// network-control priority. RFC 8622's Lower Effort codepoint joins CS1
    /// in the background category.
    pub const fn station_user_priority(self) -> WmmUserPriority {
        match self.value() {
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

const ETHERNET_HEADER_LEN: usize = 14;
const ETHER_TYPE_IPV4: u16 = 0x0800;
const ETHER_TYPE_VLAN: u16 = 0x8100;
const ETHER_TYPE_PROVIDER_VLAN: u16 = 0x88a8;
const ETHER_TYPE_IPV6: u16 = 0x86dd;

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
    let mut reader = oer_wire::Reader::new(ethernet);
    let Ok(&[.., ether_type_high, ether_type_low]) = reader.array::<ETHERNET_HEADER_LEN>() else {
        return WmmTrafficClass::BEST_EFFORT;
    };
    let payload = reader.remaining();

    let ether_type = u16::from_be_bytes([ether_type_high, ether_type_low]);
    if matches!(ether_type, ETHER_TYPE_VLAN | ETHER_TYPE_PROVIDER_VLAN) {
        let Some(&[tci_high, tci_low, _, _]) = payload.first_chunk::<4>() else {
            return WmmTrafficClass::BEST_EFFORT;
        };
        let tci = VlanTci::from_bytes([tci_high, tci_low]);
        let user_priority = WmmUserPriority::from_code(tci.pcp());
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
            Dscp(DsField::from_bytes([services]).dscp())
        }
        ETHER_TYPE_IPV6 => {
            let Some(&[version_and_class, class_and_label]) = payload.first_chunk::<2>() else {
                return WmmTrafficClass::BEST_EFFORT;
            };
            let prefix = Ipv6HeaderPrefix::from_bytes([version_and_class, class_and_label]);
            if prefix.version() != 6 {
                return WmmTrafficClass::BEST_EFFORT;
            }
            Dscp(DsField::from_bytes([prefix.traffic_class()]).dscp())
        }
        _ => return WmmTrafficClass::BEST_EFFORT,
    };
    let user_priority = dscp.station_user_priority();
    WmmTrafficClass::from_user_priority(user_priority, WmmClassificationSource::Dscp(dscp))
}

oer_wire::bitfield! {
    /// IPv4's first octet: version and Internet Header Length.
    /// SOURCE: RFC 791, 3.1 (Internet Header Format).
    pub struct Ipv4VersionIhl[1] (little) {
        pub ihl, with_ihl: u8 = 0..4;
        pub version, with_version: u8 = 4..8;
    }
}

oer_wire::bitfield! {
    /// Differentiated Services field, also carried by IPv6 Traffic Class.
    /// SOURCE: RFC 2474, 3 (Differentiated Services Field).
    pub struct DsField[1] (little) {
        /// SOURCE: RFC 3168, 5 (Explicit Congestion Notification in IP).
        pub ecn, with_ecn: u8 = 0..2;
        pub dscp, with_dscp: (u8) = 2..8;
        /// Three DSCP MSBs used by the default priority mapping.
        /// SOURCE: RFC 2474, 4.2.2 (Class Selector Codepoints).
        pub class_selector, with_class_selector: u8 = 5..8, view;
    }
}

oer_wire::bitfield! {
    /// First two IPv6 header octets, including the complete Traffic Class.
    /// SOURCE: RFC 8200, 3 (IPv6 Header Format).
    pub struct Ipv6HeaderPrefix[2] (big) {
        pub flow_label_high, with_flow_label_high: u8 = 0..4;
        pub traffic_class, with_traffic_class: u8 = 4..12;
        pub version, with_version: u8 = 12..16;
    }
}

oer_wire::bitfield! {
    /// VLAN tag control information, including Priority Code Point.
    /// SOURCE: IEEE Std 802.1Q-2022, 9.6 (Tag Control Information).
    pub struct VlanTci[2] (big) {
        pub vid, with_vid: u16 = 0..12;
        pub dei, with_dei: bool = 12..13;
        pub pcp, with_pcp: u8 = 13..16;
    }
}

#[cfg(test)]
mod tests;
