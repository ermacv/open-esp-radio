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

//! Chip-independent Wi-Fi execution primitives shared by chip runtimes.
//!
//! These async ownership contracts bind no executor. Waiting on time uses
//! `oer_time::Timer`; bounded channels and task signals use `embassy-sync`.
//!
//! [`configuration`] validates the requested Wi-Fi owner topology against a
//! backend's MAC service capabilities.
//! [`monitor`] provides bounded capture and injection handoffs. [`connected_tasks`]
//! coordinates finite task shutdown, and [`station_network`] retains network
//! ownership across station associations.
//! [`station_service`] is the station's thin service on a lower-MAC port:
//! one waiting point, every machine's actions after each input. Hardware access and concrete
//! task assembly belong to the chip backend and its integration.
//!
//! The station's SPSC sender waits through `ready(&mut self)`, returning a
//! [`station_service::SendPermit`] that holds its exclusive producer borrow.
//! `send` publishes without a fallible second capacity check. Cancelling the
//! wait or dropping the permit consumes neither a value nor queue capacity;
//! the receiver may run on another thread.
//!
pub mod configuration;
pub mod connected_tasks;
pub mod monitor;
pub mod station_network;
pub mod station_service;
pub mod survey;
pub mod vif;

pub use monitor::injection as monitor_injection;
pub use monitor::{
    MonitorCaptureFrame, MonitorCaptureMetadata, MonitorCapturePool, MonitorCaptureReceiver,
    MonitorCaptureResources, MonitorCaptureSink,
};

#[cfg(test)]
extern crate std;
