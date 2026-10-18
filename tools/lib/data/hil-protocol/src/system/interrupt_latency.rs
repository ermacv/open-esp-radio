//! The latency of a software-raised interrupt at the radio interrupts'
//! level, for the event-driven backend study ().

use serde::{Deserialize, Serialize};

use postcard_schema::Schema;

/// Raise the probe's software interrupt `samples` times and time each from
/// the raise to its handler's entry, waiting `interval_micros` between
/// samples so they spread over the window the caller measures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct InterruptLatencyRequest {
    pub samples: u16,
    /// The wait after each sample; zero only yields.
    pub interval_micros: u32,
}

/// Raise-to-entry latencies in CPU cycles, waits for a busy level included.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct InterruptLatencyEvidence {
    /// The CPU clock the cycles count, in hertz.
    pub cpu_hz: u32,
    /// Samples taken; a sample whose handler did not run within the bound
    /// counts as missed instead.
    pub samples: u16,
    pub missed: u16,
    pub minimum_cycles: u32,
    pub median_cycles: u32,
    pub p99_cycles: u32,
    pub maximum_cycles: u32,
}
