//! The coexistence event priority table every supported Espressif chip
//! shares.
//!
//! Each chip's radio arbiter keeps one [`CoexPtiTable`] as shared state: the
//! vendor coexistence scheduler changes entries at run time, and every
//! protocol programs its own MAC priority fields from the values read here.
//! A priority is a request to the hardware arbiter, never a grant.

use oer_radio_coex::CoexPriority;

/// Number of coexistence events, event zero included.
pub const COEX_EVENT_COUNT: usize = 49;

/// Complete `coex_pti_tab` of esp-coex-lib
/// `c758e7b56e0fa22177a0539796e1df59978dc322`, `coexist_core.o` section
/// `.dram1.2`, 49 bytes. The bytes are equal in the pinned
/// `esp32s31/libcoexist.a` sha256
/// `13b1e1d2a1550400ddb2622648933288aee6a285d3aad454978314c4af685147`
/// (`knowledge/esp32s31/vendor/artifacts.toml`) and the ESP32-C5
/// `esp32c5/libcoexist.a` sha256
/// `de883d649e4e2e9407a6ab0dc28d5d2aa4cd8d30d0071abfe5a9183348ceb627`.
/// Index is the event number.
/// Events 1, 3, 10 and 15 are the cold Wi-Fi MAC priorities (5, 7, 3 and 1);
/// event 48 is the PHY grant-protect request (15).
const VENDOR_PTI_TABLE: [u8; COEX_EVENT_COUNT] = [
    0x0a, 0x05, 0x07, 0x07, 0x0a, 0x01, 0x01, 0x01, 0x01, 0x07, 0x03, 0x02, 0x01, 0x01, 0x01, 0x01,
    0x04, 0x09, 0x04, 0x04, 0x09, 0x04, 0x09, 0x04, 0x04, 0x05, 0x05, 0x05, 0x05, 0x04, 0x04, 0x04,
    0x04, 0x02, 0x02, 0x02, 0x0f, 0x0a, 0x04, 0x0e, 0x00, 0x0c, 0x08, 0x03, 0x01, 0x0a, 0x0a, 0x0f,
    0x0f,
];

/// One coexistence event number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexEventId(u8);

impl CoexEventId {
    /// An event of the vendor table, or `None` outside it.
    pub const fn new(value: u8) -> Option<Self> {
        if (value as usize) < COEX_EVENT_COUNT {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// The table events whose priority Wi-Fi reads (`coex_pti_get`).
pub mod wifi_events {
    use super::CoexEventId;

    /// The event whose priority Wi-Fi's power management reads on every
    /// beacon (`coex_pti_get(0)`).
    pub const BEACON: CoexEventId = CoexEventId(0);
    /// The slice event (`coex_pti_get(1)`), whose priority bounds a
    /// connection frame's scheduler priority.
    pub const SLICE: CoexEventId = CoexEventId(1);
    /// The event a connection frame requests under the reconnect policy,
    /// whose priority it then carries (`coex_pti_get(46)`).
    pub const CONNECTION: CoexEventId = CoexEventId(46);
}

/// One four-bit coexistence priority, the shared-table domain every
/// supported chip's MAC PTI fields accept.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct CoexPti(u8);

impl CoexPti {
    /// A priority of the four-bit hardware domain, or `None` above it.
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 0x0f {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The priority value.
    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Priority of every coexistence event.
///
/// The vendor coexistence scheduler changes entries at run time; the table
/// is therefore shared state of the arbiter, not a constant of any protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexPtiTable([u8; COEX_EVENT_COUNT]);

impl CoexPtiTable {
    /// The vendor cold table.
    pub const VENDOR: Self = Self(VENDOR_PTI_TABLE);

    pub const fn pti(&self, event: CoexEventId) -> CoexPti {
        // Every byte is four-bit clean: the vendor table is, and `set` takes
        // only a checked priority.
        match CoexPti::new(self.0[event.0 as usize]) {
            Some(pti) => pti,
            None => panic!("the priority table holds only four-bit values"),
        }
    }

    pub fn set(&mut self, event: CoexEventId, pti: CoexPti) {
        self.0[event.0 as usize] = pti.value();
    }

    /// The priority of an IEEE 802.15.4 coexistence level.
    pub const fn ieee802154_pti(&self, level: Ieee802154CoexLevel) -> CoexPti {
        self.pti(level.event())
    }

    pub const fn as_bytes(&self) -> &[u8; COEX_EVENT_COUNT] {
        &self.0
    }
}

/// IEEE 802.15.4 coexistence priority level (`ieee802154_coex_event_t`).
///
/// The pinned archives' `coex_ieee802154_pti_get` reads the priority of
/// level `n` from the shared table at event `40 + n` (esp-coex-lib
/// `c758e7b56e0fa22177a0539796e1df59978dc322`, `coexist_api.o`, whose body is
/// equal in `esp32s31/libcoexist.a` sha256
/// `13b1e1d2a1550400ddb2622648933288aee6a285d3aad454978314c4af685147` and
/// `esp32c5/libcoexist.a` sha256
/// `de883d649e4e2e9407a6ab0dc28d5d2aa4cd8d30d0071abfe5a9183348ceb627`);
/// `esp_coex_ieee802154_txrx_pti_set` and `esp_coex_ieee802154_ack_pti_set`
/// publish that priority unchanged to the MAC's TX/RX and ACK PTI fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Ieee802154CoexLevel {
    High = 1,
    Middle = 2,
    Low = 3,
    Idle = 4,
}

impl Ieee802154CoexLevel {
    /// The shared-table event that holds this level's priority.
    pub const fn event(self) -> CoexEventId {
        CoexEventId(40 + self as u8)
    }

    /// The level of a portable priority: the idle scene, the ordinary low
    /// level, the middle level and the high level, in order.
    pub const fn from_priority(priority: CoexPriority) -> Self {
        match priority {
            CoexPriority::Idle => Self::Idle,
            CoexPriority::Normal => Self::Low,
            CoexPriority::Elevated => Self::Middle,
            CoexPriority::Critical => Self::High,
        }
    }

    /// The portable priority of this level.
    pub const fn priority(self) -> CoexPriority {
        match self {
            Self::Idle => CoexPriority::Idle,
            Self::Low => CoexPriority::Normal,
            Self::Middle => CoexPriority::Elevated,
            Self::High => CoexPriority::Critical,
        }
    }
}

impl From<CoexPriority> for Ieee802154CoexLevel {
    fn from(priority: CoexPriority) -> Self {
        Self::from_priority(priority)
    }
}

impl From<Ieee802154CoexLevel> for CoexPriority {
    fn from(level: Ieee802154CoexLevel) -> Self {
        level.priority()
    }
}

#[cfg(test)]
mod tests;
