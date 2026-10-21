//! Audited ESP32-S31 Wi-Fi MAC DMA ownership boundary.
//!
//! This module owns the chip descriptor geometry, finite RX walker operations,
//! live ring state and permanently located RX arena. Protocol decoding and
//! executor policy deliberately remain above this leaf.

pub mod descriptor;
#[cfg(test)]
mod descriptor_publish_tests;
pub mod ring_index;
pub mod rx_dma;
pub mod rx_ring;
pub mod rx_storage;
pub mod space;
pub mod tx_ampdu_storage;
pub mod tx_storage;

/// Place one qualified RX hot-path item in executable internal RAM for performance.
///
/// Rust 2024 makes section placement an unsafe attribute because an arbitrary
/// section can violate platform invariants. This chip leaf owns that invariant:
/// the board linker maps `.hot.text.*` to aligned executable SRAM, and the macro
/// is intentionally limited to code items rather than storage.
#[macro_export]
macro_rules! place_rx_hot_path {
    ($(#[$attribute:meta])* $visibility:vis fn $name:ident $($body:tt)*) => {
        $(#[$attribute])*
        #[cfg_attr(
            target_arch = "riscv32",
            unsafe(link_section = ".hot.text.open_radio_rx_hot")
        )]
        $visibility fn $name $($body)*
    };
}
