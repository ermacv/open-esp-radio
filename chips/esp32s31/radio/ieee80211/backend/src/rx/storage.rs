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
/// A wait between stopped-ring publication and walker enable, as policy: no
/// source states it, and nothing confirms that the hardware needs it. The
/// vendor neither waits nor polls there. SOURCE: pinned
/// libnet80211[ieee80211_ioctl.o]::wifi_hw_start +0xea..+0xf6: hal_init +0x94
/// mac_rxbuf_init publishes RX_DESCRIPTOR_BASE, chip_enable +0x2a ic_enable_rx
/// sets WALKER_ENABLE; no delay or poll between them, and that interval has no
/// stated length.
pub const ESP32S31_RX_WALKER_ENABLE_SETTLE_US: u32 = 5;

/// Wait the walker-enable settle on `timer`. A settle past the timer's range
/// never ends, so the walker is never started early.
pub async fn walker_enable_settle(timer: &impl oer_portable_time::Timer) {
    match timer.deadline_after(oer_portable_time::Duration::from_micros(u64::from(
        ESP32S31_RX_WALKER_ENABLE_SETTLE_US,
    ))) {
        Ok(deadline) => timer.wait_until(deadline).await,
        Err(oer_portable_time::TimeOverflow) => core::future::pending().await,
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
    S = ChipDma,
> = RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE, S>;
