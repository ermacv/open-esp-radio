#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! ESP32-S31 SoC services backed by esp-hal peripheral witnesses.
//!
//! Radio registers belong to `oer-esp32s31-pac`. This crate owns
//! esp-hal singleton witnesses for non-radio platform blocks. Cache and flash
//! modules provide typed register operations; the DMA module additionally owns
//! transfer memory, descriptors and interrupt-driven completion. These services
//! are separate from the generated radio PAC and from radio protocol policy.

mod cache;
#[cfg(any(test, feature = "axi-gdma-mem2mem"))]
mod dma;
#[cfg(feature = "esp32s31")]
pub mod entropy;
#[cfg(feature = "esp32s31")]
mod flash;
#[cfg(feature = "esp32s31")]
mod reset;
pub mod watchdog;

#[cfg(feature = "esp32s31")]
pub use reset::reset_system;

#[cfg(feature = "axi-gdma-mem2mem")]
pub use cache::maintenance::{PsramCacheWritebackError, writeback_psram_for_dma_read};
#[cfg(feature = "esp32s31")]
pub use cache::performance::L1CachePerformanceCounters;
pub use cache::performance::{
    L1CacheBusSnapshot, L1CacheCounterEnable, L1CachePerformanceSnapshot,
};
#[cfg(feature = "axi-gdma-mem2mem")]
pub use dma::mem2mem::{
    AxiGdmaDescriptor, AxiGdmaMem2Mem, AxiGdmaMem2MemChannel, AxiGdmaMem2MemError,
    AxiGdmaMem2MemPrepared, AxiGdmaMem2MemPreparedOwner, AxiGdmaMem2MemReport,
    AxiGdmaMem2MemRxStatus, AxiGdmaMem2MemSegment, AxiGdmaMem2MemSegmentsPrepared,
    AxiGdmaMem2MemSegmentsTransfer, AxiGdmaMem2MemStatus, AxiGdmaMem2MemTransfer,
    AxiGdmaMem2MemTransferError, AxiGdmaMem2MemTransferOwner, AxiGdmaMem2MemTxStatus, BurstSize,
    axi_gdma_mem2mem_interrupt,
};
#[cfg(feature = "esp32s31")]
pub use flash::mmu::{FLASH_XIP_END, FLASH_XIP_START, FlashMmu};
