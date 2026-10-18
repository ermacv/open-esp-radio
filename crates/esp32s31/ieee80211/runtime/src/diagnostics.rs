//! Optional, non-owning observation boundaries.
//!
//! Observers receive value-only events and cannot influence datapath
//! scheduling, protocol state, MMIO ownership or resource lifecycles.

#[cfg(any(feature = "diagnostics", test))]
pub mod access_point;
pub mod aggregate_tx;
#[cfg(feature = "task-poll-telemetry")]
pub mod core0_ap_rx_cycles;
#[cfg(not(feature = "task-poll-telemetry"))]
#[path = "diagnostics/disabled/core0_ap_rx_cycles.rs"]
pub(crate) mod core0_ap_rx_cycles;
pub(crate) mod core0_paths;
#[cfg(feature = "task-poll-telemetry")]
pub mod core0_rx_cycles;
#[cfg(not(feature = "task-poll-telemetry"))]
#[path = "diagnostics/disabled/core0_rx_cycles.rs"]
pub(crate) mod core0_rx_cycles;
pub mod core0_rx_performance;
#[cfg(feature = "task-poll-telemetry")]
pub mod core0_rx_reorder_cycles;
#[cfg(not(feature = "task-poll-telemetry"))]
#[path = "diagnostics/disabled/core0_rx_reorder_cycles.rs"]
pub(crate) mod core0_rx_reorder_cycles;
#[cfg(feature = "task-poll-telemetry")]
pub mod core0_rx_service_histogram;
#[cfg(not(feature = "task-poll-telemetry"))]
#[path = "diagnostics/disabled/core0_rx_service_histogram.rs"]
pub(crate) mod core0_rx_service_histogram;
#[cfg(feature = "diagnostics")]
pub mod network;
pub(crate) mod profile;
pub mod rx_pipeline;
