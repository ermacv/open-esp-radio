//! Qualified storage geometry and aliases over the affine DMA arena.

use oer_esp32s31_ieee80211_dma::rx_storage::{RxDmaBuffer, RxDmaStorage};

/// Descriptor count and allocation geometry qualified by the ordinary S31
/// large-RX profile.
pub const ESP32S31_RX_DESCRIPTOR_COUNT: usize = 96;
pub const ESP32S31_RX_BUFFER_SIZE: usize = 4_608;
pub const ESP32S31_RX_BUFFER_STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_SIZE + 4;
/// Platform settle edge between stopped-ring publication and walker enable.
pub const ESP32S31_RX_WALKER_ENABLE_SETTLE_US: u32 = 5;

/// Wait the walker-enable settle on `timer`. A settle past the timer's range
/// never ends, so the walker is never started early.
pub async fn walker_enable_settle(timer: &impl oer_time::Timer) {
    match timer.deadline_after(oer_time::Duration::from_micros(u64::from(
        ESP32S31_RX_WALKER_ENABLE_SETTLE_US,
    ))) {
        Ok(deadline) => timer.wait_until(deadline).await,
        Err(oer_time::TimeOverflow) => core::future::pending().await,
    }
}

/// Qualified large-RX profile aliases over the executor-independent MAC arena.
pub type ReceiveDmaBuffer<
    const BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> = RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>;

pub type ReceiveDmaStorage<
    const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT,
    const BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> = RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE>;
