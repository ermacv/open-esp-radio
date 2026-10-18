//! The station scan values and primitive port.
//!
//! They are the portable ones of `oer_ieee80211_sta::scan`; the channel-visit
//! transaction over a [`StaScanPort`] is the `StaScanBackend` of
//! `oer-ieee80211-sta-service`, which the ESP32-S31 runtime implements the
//! port for.

pub use oer_ieee80211_sta::scan::{
    ActiveProbeOutcome, StaScanConfig, StaScanConfigError, StaScanError, StaScanPort,
};
