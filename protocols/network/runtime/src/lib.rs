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

mod service;
mod socket;

pub use service::{NotStarted, Service, StopLane};
pub use socket::{
    ForeignObserver, Lanes, Observer, Refusal, Refused, Sent, ServiceEnd, Socket, SocketClosed,
    SocketLanes, lanes,
};

#[cfg(test)]
mod tests;
