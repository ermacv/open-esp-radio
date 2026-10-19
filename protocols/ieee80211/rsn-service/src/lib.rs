#![no_std]
#![forbid(unsafe_code)]

//! Executor-independent drivers of the WPA2-Personal station handshake.
//!
//! `oer-ieee80211-rsn` owns the sans-IO supplicant, its typed requests and
//! the ports its drivers wait on. This crate owns those drivers: the
//! four-way-handshake runner with its absolute response deadlines, the
//! key-install runner with its rollback ordering, and the asynchronous
//! key-data unwrap around the supplicant. Time enters through the `oer-time`
//! [`Timer`](oer_time::Timer) port.

#[cfg(test)]
extern crate std;

pub mod runner;
pub mod supplicant;
