//! The station and the network end to end on the host: the real station
//! lifecycle, cores and datapath actor over the model's data port, the
//! station's network end, the network core, its service and a socket,
//! against a simulated access point with a DHCP server and a peer behind it,
//! all under virtual time ([`simulation`]).

#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    reason = "a host test"
)]

mod simulation;

mod addressing;
mod join;
mod link;
mod queued;
mod rx_cycle;
mod slow_consumer;
mod stop;
mod traffic;
