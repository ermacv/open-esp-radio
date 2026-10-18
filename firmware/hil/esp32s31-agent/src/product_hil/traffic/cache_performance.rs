//! Diagnostic-only ESP32-S31 L1 cache performance values.
//!
//! Register ownership and access stay in the chip platform PAC. The HIL only
//! transports semantic snapshots into its evidence report.

pub(in crate::product_hil) use oer_esp32s31_soc_esp_hal::L1CachePerformanceSnapshot;
