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
//! Frozen, bounded radio observations. No message starts an independent RF measurement.
use oer_esp32s31_instrumentation::health::*;
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

pub const COUNTER_PAGE: usize = 16;
pub const SIGNAL_PAGE: usize = 8;
pub const DETAIL_PAGE: usize = 2;
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
pub struct HealthPageRequest {
    pub identity: HealthIdentity,
    pub first: u16,
    pub count: u8,
}
impl HealthPageRequest {
    pub fn range(self, total: usize, limit: usize) -> Option<core::ops::Range<usize>> {
        let first = usize::from(self.first);
        let count = usize::from(self.count);
        let end = first.checked_add(count)?;
        (count > 0 && count <= limit && end <= total).then_some(first..end)
    }
}
macro_rules! page {
    ($request:ident, $page:ident, $record:ty, $limit:ident) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
        pub struct $request(pub HealthPageRequest);
        #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
        pub struct $page {
            pub identity: HealthIdentity,
            pub first: u16,
            pub total: u16,
            pub records: heapless::Vec<$record, $limit>,
        }
    };
}
page!(ReadHealthCounters, CounterPage, Counter, COUNTER_PAGE);
page!(ReadHealthSignals, SignalPage, Signal, SIGNAL_PAGE);
page!(ReadHealthTracking, TrackingPage, Tracking, DETAIL_PAGE);
page!(
    ReadHealthMeasurements,
    MeasurementPage,
    Measurement,
    DETAIL_PAGE
);
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
    endpoint ReadHealthCounters = "health/counters/read" => CounterPage;
    topic CounterPage = "health/counters";
    endpoint ReadHealthSignals = "health/signals/read" => SignalPage;
    topic SignalPage = "health/signals";
    endpoint ReadHealthTracking = "health/tracking/read" => TrackingPage;
    topic TrackingPage = "health/tracking";
    endpoint ReadHealthMeasurements = "health/measurements/read" => MeasurementPage;
    topic MeasurementPage = "health/measurements";
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
