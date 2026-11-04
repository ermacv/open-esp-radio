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

//! Owned-packet network boundary for the optimized Embassy/Xarxa integration.
//!
//! This crate transfers general-memory packet owners between the network stack
//! and the radio. It contains no Wi-Fi scheduling, physical SRAM allocator or
//! compatibility implementation of the released Embassy driver API. RX frames
//! are copied into packet-pool storage before publication to the network stack.
//! [`ethernet`] consumes the neutral `oer_protocols_network_port::NetDevice` contract; a
//! a frame the stack sends goes straight to the device's transmit half.
//! [`ip`] builds one permanent stack over that device and publishes its typed
//! status across associations.

pub use oer_protocols_network_port::{
    EthernetFrameView, FrameLengthError, LinkState, NetTransmit, NetworkInterfaceId,
    RxEnqueueError, TransmitError,
};

pub mod ethernet;
pub mod ip;
mod owned;
#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    reason = "a test fixture's counts"
)]
mod sink;

pub use owned::{
    EndpointSplitError, OwnedEndpointResources, OwnedLinkController, OwnedNetworkDevice,
    OwnedNetworkRunner, OwnedNetworkTxFrame, OwnedRxPublisher,
};
