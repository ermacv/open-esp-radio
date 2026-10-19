//! IEEE 802.15.4 MAC owners and the ESP32-C5 data of the chip-neutral engine.

pub mod ll;

pub mod mac;

pub mod tx_power;

/// The memory the ESP32-C5 IEEE 802.15.4 MAC DMA reaches: the internal SRAM
/// of `SOC_DMA_LOW`..`SOC_DMA_HIGH` in ESP-IDF's
/// `soc/esp32c5/include/soc/soc.h` at
/// ``. The engine checks its frame
/// buffers against it
/// ([`oer_espressif_ieee802154_engine::engine::Ieee802154Engine::buffers_dma_visible`]).
pub const IEEE802154_DMA_WINDOW: core::ops::Range<usize> = 0x4080_0000..0x4086_0000;

pub use tx_power::ESP32C5_TX_POWER_LEVELS;
