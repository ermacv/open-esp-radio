#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented,
        clippy::disallowed_macros,
        clippy::disallowed_methods,
    )
)]

//! The asynchronous glue between the Wi-Fi lower-MAC port and the sans-IO
//! cores: it waits on port events, `oer_time::Timer` deadlines and the
//! application's commands, hands each input to the cores and carries out
//! their actions through the port. It decides no protocol question and
//! neither parses nor builds frames; it binds no executor.
//!
//! [`station_service`] is the station's service on a lower-MAC port: one
//! waiting point, every machine's actions after each input. [`monitor`]
//! provides bounded capture and injection handoffs. Hardware access and
//! concrete task assembly belong to the chip backend and its integration.
//!
//! The station's SPSC sender waits through `ready(&mut self)`, returning a
//! `oer_memory::WakingSpscSendPermit` that holds its exclusive producer borrow.
//! `send` publishes without a fallible second capacity check. Cancelling the
//! wait or dropping the permit consumes neither a value nor queue capacity;
//! the receiver may run on another thread.

pub mod monitor;
pub mod station_service;

pub use monitor::injection as monitor_injection;
pub use monitor::{
    MonitorCaptureFrame, MonitorCaptureMetadata, MonitorCapturePool, MonitorCaptureReceiver,
    MonitorCaptureResources, MonitorCaptureSink,
};

#[cfg(test)]
extern crate std;
