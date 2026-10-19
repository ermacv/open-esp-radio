//! Coexistence priorities the ESP32-C5 radio clients publish.
//!
//! The table is the vendor cold table; the arbiter that changes it at run
//! time is not owned by this crate yet.

/// The event priority table and IEEE 802.15.4 levels every Espressif chip
/// shares.
pub use oer_espressif_coex::{
    COEX_EVENT_COUNT, CoexEventId, CoexPti, CoexPtiTable, Ieee802154CoexLevel,
};
