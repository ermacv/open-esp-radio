#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
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
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods,
        clippy::indexing_slicing,
        clippy::unwrap_used
    )
)]

//! The network core's task and its sockets over messages (see the README).
//!
//! Reading a result consumes its observer, so it cannot read a credit's next
//! command:
//!
//! ```compile_fail,E0382
//! fn twice(observer: oer_protocols_network_runtime::Observer<'_>) {
//!     let _first = observer.take();
//!     let _second = observer.take();
//! }
//! ```

mod device;
mod diagnostics;
mod local;
mod observation;
mod returns;
mod service;
mod socket;
mod watch;

pub use device::DevicePort;
pub use diagnostics::{Diagnosis, Diagnostics, STAGES, Stage};
pub use local::{
    Interest, LocalEnd, LocalRefused, Next, Offer, Produce, Producer, Producers, Work,
};
pub use observation::{LocalEvent, PublishEnd, RequestKind, ServiceEvent, TurnEnd};
pub use returns::{NotHome, ReturnEnd, ReturnLane, ReturnRefusal, Returner};
pub use service::{
    BindRefused, EndCause, Ended, MemoryPolicy, Service, StopLane, serve_late_loans,
};
pub use socket::{
    LateSocket, LendingLanes, LendingSocket, Lent, Observer, RESERVE_AHEAD, Received, Refusal,
    Refused, Sent, Socket, SocketClosed, SocketLanes,
};
pub use watch::{ConfigurationWatch, WatchRefused, Watched};

#[cfg(test)]
mod tests;
