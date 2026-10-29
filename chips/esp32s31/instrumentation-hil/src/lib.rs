#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods
    )
)]
//! Radio observation lifecycle and the generic capture reader binding. No message starts an independent RF measurement.
use oer_esp32s31_instrumentation::health::*;
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

pub const MAX_RECORDS: usize = 1024;
pub const PSRAM_BUDGET: usize = 128 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct BeginHealth {
    pub channel: u8,
    pub peer: [u8; 6],
    /// Configured peer PHY rate, not a claim about received frames.
    pub peer_rate_kbps: Option<u32>,
    /// Configured peer power, not measured antenna power.
    pub peer_power_dbm: Option<i8>,
}
impl BeginHealth {
    pub const fn valid(self) -> bool {
        self.channel >= 1 && self.channel <= 14
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HealthIdentity {
    pub observation: u32,
    pub epoch: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HealthStarted {
    pub observation: u32,
    pub boot: u64,
    pub capacity: u16,
    pub storage_bytes: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RunHealthWindow(pub HealthIdentity);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HealthWindow {
    pub identity: HealthIdentity,
    pub started_micros: u64,
    pub finished_micros: u64,
    pub rx_cleaned_up: bool,
    pub tracking_failed: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct FinishHealth(pub HealthIdentity);
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Counts {
    pub counters: u16,
    pub signals: u16,
    pub tracking: u16,
    pub measurements: u16,
}
impl Counts {
    /// The exact count, or `None` if the host's `usize` cannot hold it.
    pub const fn total(self) -> Option<usize> {
        let Some(total) = (self.counters as usize).checked_add(self.signals as usize) else {
            return None;
        };
        let Some(total) = total.checked_add(self.tracking as usize) else {
            return None;
        };
        total.checked_add(self.measurements as usize)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HealthFinished {
    pub identity: HealthIdentity,
    pub counts: Counts,
    pub dropped: u32,
    pub failed: bool,
    pub rx_cleaned_up: bool,
    pub rf_closed: bool,
}
impl HealthFinished {
    pub const fn complete(self) -> bool {
        self.dropped == 0 && !self.failed && self.rx_cleaned_up && self.rf_closed
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReleaseHealth(pub HealthIdentity);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HealthReleased(pub HealthIdentity);

oer_hil_protocol::messages! {
    endpoint BeginHealth = "health/begin" => HealthStarted;
    topic HealthStarted = "health/started";
    endpoint RunHealthWindow = "health/window" => HealthWindow;
    topic HealthWindow = "health/window-finished";
    endpoint FinishHealth = "health/finish" => HealthFinished;
    topic HealthFinished = "health/finished";
    endpoint ReleaseHealth = "health/release" => HealthReleased;
    topic HealthReleased = "health/released";
}
#[cfg(test)]
mod tests;

/// Append-only capture storage; publication freezes all pages.
pub mod store;

/// Reviewed paths, schemas (through keys) and message kinds.
#[cfg(feature = "registry")]
pub const MESSAGES_LOCK: &str = include_str!("../messages.lock");

/// Full postcard schema of records served through the generic capture service.
pub const RECORD_SCHEMA: oer_hil_protocol::Key =
    oer_hil_protocol::Key::of::<Record>("esp32s31/health/record");

/// The lifecycle dictionary together with the generic service its reader
/// requires. Hosts must discover both before starting a health observation.
pub const REQUIRED_KEYS: &[oer_hil_protocol::Key] = &{
    use oer_hil_agent::base::Service;
    const GROUPS: &[&[oer_hil_protocol::Key]] = &[KEYS, oer_hil_agent::capture::Request::KEYS];
    const LEN: usize = KEYS.len() + oer_hil_agent::capture::Request::KEYS.len();
    match oer_hil_protocol::base::concat_keys::<LEN>(GROUPS) {
        Some(keys) => keys,
        None => panic!("health capture requirements contain both mounted services"),
    }
};
