#![no_std]

//! ESP32-S31 qualification-only observers and report accumulators.
//!
//! Driver crates publish typed observation events; this crate selects
//! counters, sampling policy and report snapshots for HIL. Production
//! firmware does not depend on it.

#[cfg(test)]
extern crate std;

pub mod aggregate_tx;
pub mod airtime;
pub mod mac_irq;
pub mod rx_delivery;
pub mod rx_evidence;
#[cfg(feature = "rx-ownership-observation")]
pub mod rx_ownership;
pub mod rx_pipeline;
pub mod task_poll;
#[cfg(any(feature = "tx-wait-probe", test))]
pub mod tx_wait;
pub mod udp_rx_window;
pub mod wait;

pub mod rx_rejection;
pub mod tx_retention;

pub mod tx_progress;

pub mod rx_anomaly;

pub mod arp_frontier;
