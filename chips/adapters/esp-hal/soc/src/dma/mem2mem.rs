//! Minimal ESP32-S31 AXI-GDMA memory-to-memory owner.
//!
//! This deliberately implements only the transfer required by the Wi-Fi TX
//! promotion path: cached PSRAM to uncached internal SRAM. Long-lived software
//! queues and generic peripheral DMA policy do not belong in this driver.

#[cfg(feature = "axi-gdma-mem2mem")]
#[allow(
    unsafe_code,
    reason = "this reviewed interrupt boundary places the handler and its waker in the required linker sections"
)]
mod completion;
mod descriptor;
#[cfg(feature = "axi-gdma-mem2mem")]
#[allow(
    unsafe_code,
    reason = "this reviewed PAC boundary programs validated S31 register fields and emits the DMA visibility fence"
)]
mod registers;
mod status;
#[cfg(feature = "axi-gdma-mem2mem")]
mod transfer;

#[cfg(feature = "axi-gdma-mem2mem")]
pub use completion::channel0_interrupt as axi_gdma_mem2mem_interrupt;
#[cfg(feature = "axi-gdma-mem2mem")]
pub use descriptor::{AxiGdmaDescriptor, BurstSize};
#[cfg(feature = "axi-gdma-mem2mem")]
pub use status::{AxiGdmaMem2MemRxStatus, AxiGdmaMem2MemStatus, AxiGdmaMem2MemTxStatus};
#[cfg(feature = "axi-gdma-mem2mem")]
pub use transfer::*;
