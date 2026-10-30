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
//! [`ethernet`] consumes the neutral `oer_network_port::NetDevice` contract; a
//! refused TX retains its original packet and software credit together.
//! [`ip`] builds one permanent stack over that device and publishes its typed
//! status across associations. [`datapath`] holds the software/physical TX
//! ownership contracts and the destination/flow queue arena between them.

pub use oer_network_port::{
    ETHERNET_HEADER_LEN, FrameLengthError, LinkState, NetworkInterfaceId, RxEnqueueError,
};

pub mod datapath;
pub mod ethernet;
pub mod ip;
mod owned;

pub use owned::{
    AccessRefused, EndpointSplitError, OwnedEndpointResources, OwnedLinkController,
    OwnedNetworkDevice, OwnedNetworkRunner, OwnedNetworkTxFrame, OwnedRxPublisher,
    OwnedTxFrameSource,
};

impl datapath::SoftwareTxFrame for OwnedNetworkTxFrame<'_> {
    fn interface(&self) -> NetworkInterfaceId {
        OwnedNetworkTxFrame::interface(self)
    }

    fn ethernet(&self) -> &[u8] {
        OwnedNetworkTxFrame::ethernet(self)
    }
}
