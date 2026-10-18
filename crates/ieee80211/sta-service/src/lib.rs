#![no_std]
#![forbid(unsafe_code)]

//! Executor- and chip-independent Wi-Fi station drivers.
//!
//! `oer-ieee80211-sta` owns the station state machines, policy and the ports
//! its drivers wait on. This crate owns those drivers: the join runner that
//! orders Authentication and Association transactions against absolute
//! deadlines, the finite candidate scan and the outer attempt, reconnect and
//! backoff lifecycle. Each driver is a future any executor can poll; time
//! enters through the `oer-time` [`Timer`](oer_time::Timer) port.

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod test_support;

pub mod attempt;
pub mod join;
pub mod scan;
pub mod station;
