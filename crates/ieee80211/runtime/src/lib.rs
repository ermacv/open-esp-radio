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
    clippy::disallowed_macros
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
    )
)]

//! Chip-independent Wi-Fi execution primitives shared by chip runtimes.
//!
//! These async ownership contracts bind no executor. Waiting on time uses
//! `oer_time::Timer`; bounded channels and task signals use `embassy-sync`.
//!
//! [`monitor`] provides bounded capture and injection handoffs. [`connected_tasks`]
//! coordinates finite task shutdown, and [`station_network`] retains network
//! ownership across station associations. [`stack_boundary`] provides an explicit
//! poll boundary for large async state machines.
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
//! The `test-support` feature exposes `lower_mac_owner`, the in-memory backend
//! and conformance harness for tests of port consumers. The module is absent
//! from production builds without that feature. The feature build includes
//! the same bounded continuous RX and terminal fault injection as unit tests.
//! It requires `alloc`, including on RV32, but only the host test harness
//! requires `std`.
#![cfg_attr(
    not(feature = "test-support"),
    doc = r"
```compile_fail,E0432
use oer_ieee80211_runtime::lower_mac_owner;
```

Production code uses the station service directly:

```
use oer_ieee80211_runtime::station_service::StationService;
```

"
)]
#![cfg_attr(
    feature = "test-support",
    doc = r"
With `test-support`, host fixtures can name the backend's capabilities:

```
use oer_ieee80211_runtime::lower_mac_owner::model::ModelCaps;
```

"
)]

pub mod connected_tasks;
#[cfg(any(test, feature = "test-support"))]
pub mod lower_mac_owner;
pub mod monitor;
pub mod stack_boundary;
pub mod station_network;
pub mod station_service;

pub use monitor::injection as monitor_injection;
pub use monitor::{
    MonitorCaptureFrame, MonitorCaptureMetadata, MonitorCapturePool, MonitorCaptureReceiver,
    MonitorCaptureResources, MonitorCaptureSink,
};

#[cfg(test)]
extern crate std;

// The in-memory lower-MAC model allocates its state and events.
#[cfg(any(test, feature = "test-support"))]
extern crate alloc;
