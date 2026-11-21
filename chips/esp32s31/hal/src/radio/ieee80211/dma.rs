//! Audited ESP32-S31 Wi-Fi MAC DMA ownership boundary.
//!
//! This module owns the chip descriptor geometry, finite RX walker operations,
//! live ring state and permanently located RX arena. Protocol decoding and
//! executor policy deliberately remain above this leaf.

pub mod descriptor;
#[cfg(test)]
mod descriptor_publish_tests;
pub mod receive;
pub mod ring_index;
pub mod space;
pub mod tx_ampdu_storage;
pub mod tx_storage;
