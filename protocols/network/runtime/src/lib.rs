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

mod service;
mod socket;

pub use service::{BindRefused, Direct, EndCause, Ended, Service, StopLane};
pub use socket::{LateSocket, Observer, Refusal, Refused, Sent, Socket, SocketClosed, SocketLanes};

#[cfg(test)]
mod tests;
