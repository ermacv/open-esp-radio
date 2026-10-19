//! IEEE 802.11 MAC, baseband and channel hardware operations.

pub mod baseband;

pub mod channel;

pub mod client;

/// The Wi-Fi MAC DMA: descriptors, the RX walker, its ring and arena, the TX
/// and A-MPDU storage.
pub mod dma;

pub mod mac;

#[cfg(oer_probe)]
#[doc(hidden)]
#[cfg(oer_probe)]
pub mod phy_rate;

pub mod station_wake;
