//! IEEE 802.11 MAC, baseband and channel hardware operations.

pub mod baseband;

pub mod channel;

pub mod client;

/// The Wi-Fi MAC DMA: descriptors, the RX walker, its ring and arena, the TX
/// and A-MPDU storage.
pub mod dma;

pub mod mac;

/// The Wi-Fi MAC register owners and their operations over the generated parts.
pub mod registers;

pub mod station_wake;

/// Role-neutral hardware cold-start before DMA and interrupt publication.
pub mod init;
