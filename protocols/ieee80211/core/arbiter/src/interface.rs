//! Virtual-interface identity and its binding to a channel context.
//!
//! A virtual interface is protocol state, while a channel context represents
//! one hardware radio tuning context. Keeping their identities distinct is
//! required for STA/AP concurrency: two VIFs may share one channel context
//! even when the hardware cannot tune two channels simultaneously.

use oer_ieee80211_contracts_port::{ChannelContextId, VifRole};

/// Stable identity of one virtual Wi-Fi interface within a radio owner.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct VifId(u8);

impl VifId {
    pub const PRIMARY: Self = Self(0);

    pub const fn new(index: u8) -> Self {
        Self(index)
    }

    pub const fn index(self) -> u8 {
        self.0
    }
}

/// Value-only definition of one virtual interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VirtualInterface {
    pub id: VifId,
    pub role: VifRole,
    pub address: [u8; 6],
}

impl VirtualInterface {
    pub const fn new(id: VifId, role: VifRole, address: [u8; 6]) -> Self {
        Self { id, role, address }
    }
}

/// Explicit binding between protocol state and a hardware tuning context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VifChannelBinding {
    pub vif: VifId,
    pub channel_context: ChannelContextId,
}

impl VifChannelBinding {
    pub const fn new(vif: VifId, channel_context: ChannelContextId) -> Self {
        Self {
            vif,
            channel_context,
        }
    }
}

/// One VIF together with the hardware context it currently uses.
///
/// Grouping these values avoids parallel `vif_id`/`role`/`address`/`context`
/// argument lists and makes an inconsistent binding unrepresentable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundVirtualInterface {
    pub interface: VirtualInterface,
    pub channel_context: ChannelContextId,
}

impl BoundVirtualInterface {
    pub const fn new(interface: VirtualInterface, channel_context: ChannelContextId) -> Self {
        Self {
            interface,
            channel_context,
        }
    }

    pub const fn binding(self) -> VifChannelBinding {
        VifChannelBinding::new(self.interface.id, self.channel_context)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests;
