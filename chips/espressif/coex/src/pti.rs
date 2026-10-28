//! The coexistence event priority table every supported Espressif chip
//! shares.
//!
//! Each chip's radio arbiter keeps one [`CoexPtiTable`] as shared state: the
//! vendor coexistence scheduler changes entries at run time, and every
//! protocol programs its own MAC priority fields from the values read here.
//! A priority is a request to the hardware arbiter, never a grant.

use crate::operation::ieee802154_event;
use oer_ieee802154_port::RadioOperation;

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
const VENDOR_PTI_TABLE: [u8; COEX_EVENT_COUNT] = const {
    let table = [
        0x0a, 0x05, 0x07, 0x07, 0x0a, 0x01, 0x01, 0x01, 0x01, 0x07, 0x03, 0x02, 0x01, 0x01, 0x01,
        0x01, 0x04, 0x09, 0x04, 0x04, 0x09, 0x04, 0x09, 0x04, 0x04, 0x05, 0x05, 0x05, 0x05, 0x04,
        0x04, 0x04, 0x04, 0x02, 0x02, 0x02, 0x0f, 0x0a, 0x04, 0x0e, 0x00, 0x0c, 0x08, 0x03, 0x01,
        0x0a, 0x0a, 0x0f, 0x0f,
    ];
    let mut remaining = table.as_slice();
    while let Some((priority, rest)) = remaining.split_first() {
        if *priority > 0x0f {
            panic!("the vendor table must hold four-bit priorities");
        }
        remaining = rest;
    }
    table
};

/// One coexistence event number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexEventId(EventIndex);

// One variant per table entry makes both projections total, without a
// runtime bounds check or a missing-entry fallback. The macro keeps raw
// admission and immutable/mutable projections on the same domain.
macro_rules! event_indices {
    ($($variant:ident = $value:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(u8)]
        enum EventIndex {
            $($variant = $value),+
        }

        impl CoexEventId {
            /// An event of the vendor table, or `None` outside it.
            pub const fn new(value: u8) -> Option<Self> {
                match value {
                    $($value => Some(Self(EventIndex::$variant)),)+
                    _ => None,
                }
            }

            pub const fn value(self) -> u8 {
                self.0 as u8
            }

            const fn entry<T>(self, entries: &[T; COEX_EVENT_COUNT]) -> &T {
                match self.0 {
                    $(EventIndex::$variant => &entries[$value],)+
                }
            }

            fn entry_mut<T>(self, entries: &mut [T; COEX_EVENT_COUNT]) -> &mut T {
                match self.0 {
                    $(EventIndex::$variant => &mut entries[$value],)+
                }
            }
        }
    };
}

event_indices! {
    Event0 = 0,
    Event1 = 1,
    Event2 = 2,
    Event3 = 3,
    Event4 = 4,
    Event5 = 5,
    Event6 = 6,
    Event7 = 7,
    Event8 = 8,
    Event9 = 9,
    Event10 = 10,
    Event11 = 11,
    Event12 = 12,
    Event13 = 13,
    Event14 = 14,
    Event15 = 15,
    Event16 = 16,
    Event17 = 17,
    Event18 = 18,
    Event19 = 19,
    Event20 = 20,
    Event21 = 21,
    Event22 = 22,
    Event23 = 23,
    Event24 = 24,
    Event25 = 25,
    Event26 = 26,
    Event27 = 27,
    Event28 = 28,
    Event29 = 29,
    Event30 = 30,
    Event31 = 31,
    Event32 = 32,
    Event33 = 33,
    Event34 = 34,
    Event35 = 35,
    Event36 = 36,
    Event37 = 37,
    Event38 = 38,
    Event39 = 39,
    Event40 = 40,
    Event41 = 41,
    Event42 = 42,
    Event43 = 43,
    Event44 = 44,
    Event45 = 45,
    Event46 = 46,
    Event47 = 47,
    Event48 = 48,
}

pub(crate) mod ieee802154_events {
    use super::{CoexEventId, EventIndex};

    pub const IDLE: CoexEventId = CoexEventId(EventIndex::Event44);
    pub const IMMEDIATE: CoexEventId = CoexEventId(EventIndex::Event43);
    pub const TIMED: CoexEventId = CoexEventId(EventIndex::Event42);
}

/// The table events whose priority Wi-Fi reads (`coex_pti_get`).
pub mod wifi_events {
    use super::{CoexEventId, EventIndex};

    /// The event whose priority Wi-Fi's power management reads on every
    /// beacon (`coex_pti_get(0)`).
    pub const BEACON: CoexEventId = CoexEventId(EventIndex::Event0);
    /// The slice event (`coex_pti_get(1)`), whose priority bounds a
    /// connection frame's scheduler priority.
    pub const SLICE: CoexEventId = CoexEventId(EventIndex::Event1);
    /// The event a Probe Request requests under the reconnect policy, which
    /// it only requests the air with (`pp_coex_tx_request`).
    pub const PROBE_REQUEST: CoexEventId = CoexEventId(EventIndex::Event45);
    /// The event a connection frame requests under the reconnect policy,
    /// whose priority it then carries (`coex_pti_get(46)`).
    pub const CONNECTION: CoexEventId = CoexEventId(EventIndex::Event46);
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
        // The const initializer validates the entire cold image, and the
        // only mutation accepts an already checked CoexPti.
        CoexPti(*event.entry(&self.0))
    }

    pub fn set(&mut self, event: CoexEventId, pti: CoexPti) {
        *event.entry_mut(&mut self.0) = pti.value();
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
