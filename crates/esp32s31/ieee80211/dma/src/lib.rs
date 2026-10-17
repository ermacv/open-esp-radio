#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! Audited ESP32-S31 Wi-Fi MAC DMA ownership boundary.
//!
//! This crate owns the chip descriptor geometry, finite RX walker operations,
//! live ring state and permanently located RX arena. Protocol decoding and
//! executor policy deliberately remain above this leaf.

pub mod alignment;
pub mod descriptor;
pub mod ring_index;
pub mod rx_dma;
pub mod rx_ring;
pub mod rx_storage;
pub mod tx_ampdu_storage;
pub mod tx_storage;

/// Place one qualified RX hot-path item in executable internal RAM on S31.
///
/// Rust 2024 makes section placement an unsafe attribute because an arbitrary
/// section can violate platform invariants. This chip leaf owns that invariant:
/// the board linker maps `.rwtext.*` to aligned executable SRAM, and the macro
/// is intentionally limited to code items rather than storage.
#[macro_export]
macro_rules! place_rx_hot_path {
    ($(#[$attribute:meta])* $visibility:vis fn $name:ident $($body:tt)*) => {
        $(#[$attribute])*
        #[cfg_attr(
            target_arch = "riscv32",
            unsafe(link_section = ".rwtext.open_radio_rx_hot")
        )]
        $visibility fn $name $($body)*
    };
}

/// Place a MAC interrupt publication path in executable internal SRAM.
///
/// The S31 linker maps `.rwtext.*` to executable internal RAM. As with
/// [`place_rx_hot_path`], this macro accepts only functions, never storage.
#[macro_export]
macro_rules! place_irq_path {
    ($(#[$attribute:meta])* $visibility:vis fn $name:ident $($body:tt)*) => {
        $(#[$attribute])*
        #[cfg_attr(
            target_arch = "riscv32",
            unsafe(link_section = ".rwtext.open_radio_irq")
        )]
        $visibility fn $name $($body)*
    };
}
