#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! Owned-packet network boundary for the optimized Embassy/Xarxa integration.
//!
//! This crate transfers general-memory packet owners between the network stack
//! and the radio. It contains no Wi-Fi scheduling, physical SRAM allocator or
//! compatibility implementation of the released Embassy driver API. Its only
//! unsafe code adopts detached DMA buffers as Xarxa packets for zero-copy RX.

pub use embassy_sync::blocking_mutex::raw::{NoopRawMutex, RawMutex};
pub use embassy_sync::signal::Signal;
pub use oer_network_interface::{
    ETHERNET_HEADER_LEN, FrameLengthError, LinkState, NetworkInterfaceId, RxEnqueueError,
};

mod owned;

pub use owned::{
    ExternalRxAdmission, ExternalRxCounters, ExternalRxOrigin, ExternalRxRefusal,
    OwnedEndpointResources, OwnedLinkController, OwnedNetworkDevice, OwnedNetworkRunner,
    OwnedNetworkTxFrame, OwnedRxPublisher, OwnedTxFrameSource,
};

impl<M: RawMutex> oer_ieee80211_datapath::SoftwareTxFrame for OwnedNetworkTxFrame<'_, M> {
    fn interface(&self) -> NetworkInterfaceId {
        OwnedNetworkTxFrame::interface(self)
    }

    fn ethernet(&self) -> &[u8] {
        OwnedNetworkTxFrame::ethernet(self)
    }
}
