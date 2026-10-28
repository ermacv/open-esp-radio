//! Impossible hardware inputs returned as values, retaining source observations.

use super::Ieee802154State;
use oer_espressif_ieee802154_mac_values::Ieee802154EventObservation;

/// Impossible hardware input; no selected event has been acknowledged or dispatched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// A sample includes an unnamed lane or the unsupported clock-match event.
    UnhandledEvents {
        /// Complete sampled event set, including each unnamed lane separately.
        observation: Ieee802154EventObservation,
        /// Software state at entry, before any event was handled.
        state: Ieee802154State,
    },
}
