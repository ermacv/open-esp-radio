//! Qualified storage geometry and aliases over the affine DMA arena.

use oer_chips_esp32s31_hal::radio::ieee80211::dma::{
    rx_storage::{RxDmaBuffer, RxDmaStorage},
    space::ChipDma,
};

/// Descriptor count and allocation geometry qualified by the ordinary S31
/// large-RX profile.
pub const ESP32S31_RX_DESCRIPTOR_COUNT: usize = 96;
pub const ESP32S31_RX_BUFFER_SIZE: usize = 4_608;
pub const ESP32S31_RX_BUFFER_STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_SIZE;
const _: () = assert!(
    oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_ring::rx_ring_outspans_any_unit(
        ESP32S31_RX_DESCRIPTOR_COUNT,
        ESP32S31_RX_BUFFER_SIZE,
    ),
    "one receive unit could span the whole ring"
);
/// Qualified large-RX profile aliases over the executor-independent MAC arena.
pub type ReceiveDmaBuffer<
    const BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> = RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>;

pub type ReceiveDmaStorage<
    const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT,
    const BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
    S = ChipDma,
> = RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE, S>;
