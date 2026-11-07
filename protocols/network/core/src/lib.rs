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

//! A sans-IO network core with one owner (see the crate's README).

mod block;
mod interface;
mod machine;
mod neighbor;

pub use block::{Block, BlockTooShort, Pool};
pub use interface::{Interface, InterfaceError};
pub use machine::{
    Class, CommandId, Core, Datagram, Demand, Grant, Materialized, NetEvent, OpenRefused, Outcome,
    Remote, SendRefusal, SendRefused, SocketId, StaleSocket, Unmaterialized,
};

#[cfg(test)]
mod tests;
