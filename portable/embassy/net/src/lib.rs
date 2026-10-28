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
//! [`station_network`] retains the network stack and its radio-side runner
//! across station associations. [`datapath`] holds the software/physical TX
//! ownership contracts and the destination/flow queue arena between them.

pub use embassy_sync::blocking_mutex::raw::{NoopRawMutex, RawMutex};
pub use embassy_sync::signal::Signal;
pub use oer_network::{
    ETHERNET_HEADER_LEN, FrameLengthError, LinkState, NetworkInterfaceId, RxEnqueueError,
};

pub mod datapath;
mod owned;
pub mod station_network;

pub use owned::{
    EndpointSplitError, OwnedEndpointResources, OwnedLinkController, OwnedNetworkDevice,
    OwnedNetworkRunner, OwnedNetworkTxFrame, OwnedRxPublisher, OwnedTxFrameSource,
};

impl<M: RawMutex> datapath::SoftwareTxFrame for OwnedNetworkTxFrame<'_, M> {
    fn interface(&self) -> NetworkInterfaceId {
        OwnedNetworkTxFrame::interface(self)
    }

    fn ethernet(&self) -> &[u8] {
        OwnedNetworkTxFrame::ethernet(self)
    }
}
