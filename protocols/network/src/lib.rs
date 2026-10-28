#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

//! Adapter-neutral values at the network/radio ownership boundary.
//!
//! This crate intentionally contains no queue, executor, packet allocation or
//! driver trait. Compatibility and optimized integrations map their external
//! network-stack types onto these values without making radio policy depend on
//! either integration.
//!
//! [`NetworkInterfaceId`] and [`LinkState`] describe logical publication, not
//! ownership of an Ethernet buffer. Buffer loans, queue capacity and wakeups
//! belong to the selected adapter, while DMA-visible storage remains governed
//! by `oer-memory` and the chip datapath. A successful conversion of these
//! values therefore does not imply zero-copy transfer or hardware completion.

mod flow;
pub use flow::TransportFlow;

/// Ethernet header length, excluding an FCS.
pub const ETHERNET_HEADER_LEN: usize = 14;

/// Opaque identity of one logical network endpoint sharing a physical radio.
///
/// Network adapters preserve this value but never assign Wi-Fi meaning to it.
/// The radio owner owns the mapping to STA, AP, or another VIF.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkInterfaceId(u8);

impl NetworkInterfaceId {
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Link availability observed at the radio/network boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkState {
    Down,
    Up,
}

/// Why a byte slice cannot be represented by an owned Ethernet frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameLengthError {
    /// The slice is shorter than an Ethernet header.
    TooShort,
    /// The slice exceeds the configured frame storage.
    TooLong,
}

/// Why a received frame was not admitted to a network integration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxEnqueueError {
    /// The supplied Ethernet frame length is invalid.
    InvalidLength(FrameLengthError),
    /// The fixed receive queue is full.
    QueueFull,
    /// The receive packet pool has no free owner. The radio adapter's pool
    /// notification contract determines whether to wait or drop this frame.
    PoolExhausted,
    /// The logical network interface is not active.
    LinkDown,
}
