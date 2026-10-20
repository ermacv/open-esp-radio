//! Settings, keys and lifecycle of the lower-MAC port.

/// A 48-bit IEEE MAC address in transmission order.
pub type MacAddress = [u8; 6];

/// The role of a virtual interface, which decides the frames its address
/// filter admits and the TSF it keeps.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VifRole {
    /// A non-AP station: its BSSID is the access point it joined.
    Station,
    /// An access point: its BSSID is its own address.
    AccessPoint,
}

/// A set of interface roles.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct VifRoleSet(u8);

impl VifRoleSet {
    /// No interface roles.
    pub const NONE: Self = Self(0);
    /// The non-AP station role.
    pub const STATION: Self = Self(1 << 0);
    /// The access-point role.
    pub const ACCESS_POINT: Self = Self(1 << 1);

    /// All roles present in either set.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether this set includes `role`.
    pub const fn contains(self, role: VifRole) -> bool {
        let bit = match role {
            VifRole::Station => Self::STATION.0,
            VifRole::AccessPoint => Self::ACCESS_POINT.0,
        };
        self.0 & bit != 0
    }
}

/// Which received frames the backend delivers for one interface.
///
/// The filter is a set of admission rules; a frame admitted by any rule of
/// any interface is delivered, and a frame no rule admits is not, whatever
/// superset the hardware passes. Frames addressed to the interface are
/// acknowledged by the hardware whatever the filter says about other
/// frames. The rules of [`Self::BSS_MEMBER`] belong to a BSS: a station
/// without a BSSID can request only [`Self::OTHER_BSS_MANAGEMENT`].
/// Receiving every frame regardless of address is
/// [`MonitorPhase`](crate::MonitorPhase), not a filter rule.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ReceiveFilter(u8);

impl ReceiveFilter {
    /// Nothing.
    pub const NONE: Self = Self(0);
    /// Frames whose receiver address is the interface's address.
    pub const OWN_UNICAST: Self = Self(1 << 0);
    /// Group-addressed frames of the interface's BSS.
    pub const OWN_BSS_GROUP: Self = Self(1 << 1);
    /// Beacons of the interface's BSS.
    pub const OWN_BSS_BEACONS: Self = Self(1 << 2);
    /// Beacons and Probe Responses of every BSS, as a scan needs.
    pub const OTHER_BSS_MANAGEMENT: Self = Self(1 << 3);
    /// Control frames addressed to the interface (BlockAckReq, PS-Poll).
    pub const OWN_CONTROL: Self = Self(1 << 4);
    /// Probe Requests to the interface or broadcast, carrying its BSSID or
    /// the wildcard BSSID: what an access point answers.
    pub const PROBE_REQUESTS: Self = Self(1 << 5);

    /// What a station or access point needs for its own BSS.
    pub const BSS_MEMBER: Self = Self(
        Self::OWN_UNICAST.0 | Self::OWN_BSS_GROUP.0 | Self::OWN_BSS_BEACONS.0 | Self::OWN_CONTROL.0,
    );

    /// All admission rules present in either filter.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Admission rules common to both filters.
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Whether every admission rule in `other` is present.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no admission rule is enabled.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}
