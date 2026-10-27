//! The coexistence event priority table every supported Espressif chip
//! shares.
//!
//! Each chip's radio arbiter keeps one [`CoexPtiTable`] as shared state: the
//! vendor coexistence scheduler changes entries at run time, and every
//! protocol programs its own MAC priority fields from the values read here.
//! A priority is a request to the hardware arbiter, never a grant.

use crate::operation::ieee802154_event;
use oer_ieee802154_mac::RadioOperation;

/// Number of coexistence events, event zero included.
pub const COEX_EVENT_COUNT: usize = 49;

/// Complete `coex_pti_tab` of esp-coex-lib
/// `fa152b3b6b2e3c5e3a57e55fcf4b005a4e28cef9`, `coexist_core.o` section
/// `.dram1.2`, 49 bytes. The bytes are equal in the pinned
/// `esp32s31/libcoexist.a` sha256
/// `01c1b4f2b86cbc3024a2b51f3f104ea7937fc721a4ee57b53146fea6608836bb`
/// (`chips/esp32s31/vendor/artifacts.toml`) and the ESP32-C5
/// `esp32c5/libcoexist.a` sha256
/// (esp-coex-lib `c758e7b56e0fa22177a0539796e1df59978dc322`)
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
    /// The event a Probe Request requests under the reconnect policy, which
    /// it only requests the air with (`pp_coex_tx_request`).
    pub const PROBE_REQUEST: CoexEventId = CoexEventId(45);
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

    /// The priority of an IEEE 802.15.4 operation, from the event its row
    /// names ([`ieee802154_event`]).
    pub const fn ieee802154_pti(&self, operation: RadioOperation) -> CoexPti {
        self.pti(ieee802154_event(operation))
    }

    pub const fn as_bytes(&self) -> &[u8; COEX_EVENT_COUNT] {
        &self.0
    }
}

#[cfg(test)]
mod tests;
