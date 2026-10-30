//! Exclusive station-address topology for standalone ESP-NOW operation.
//!
//! Shared Wi-Fi topology is validated by the arbiter before this module
//! narrows it to a station VIF with no AP or monitor sharing its radio owner.

use oer_ieee80211_contracts_port::ChannelContextId;
use oer_ieee80211_core_arbiter::{configuration::WifiPlan, interface::BoundVirtualInterface};

/// Capability-checked station-address VIF reserved exclusively for ESP-NOW.
///
/// This value proves that no AP or monitor role shares the radio owner. It
/// deliberately carries no association state: the station VIF supplies only
/// its individual MAC address and the sole physical channel-context identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiStandaloneEspNowPlan {
    station: BoundVirtualInterface,
}

impl WifiStandaloneEspNowPlan {
    /// Narrow a checked topology to standalone ESP-NOW ownership.
    ///
    /// This carries no scan, authentication, association, WPA2 or
    /// network-stack authority.
    pub const fn new(plan: WifiPlan) -> Option<Self> {
        if plan.access_point().is_some() || plan.monitor().is_some() {
            return None;
        }
        match plan.station() {
            Some(station) => Some(Self { station }),
            None => None,
        }
    }

    pub const fn station(self) -> BoundVirtualInterface {
        self.station
    }

    pub const fn channel_context(self) -> ChannelContextId {
        self.station.channel_context
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests;
