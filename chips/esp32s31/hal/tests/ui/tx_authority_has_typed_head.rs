#![no_std]
use oer_esp32s31_hal::layouts::{DmaRef, WifiTxDmaDescriptor};
use oer_memory::{HardwareOwnedTxDma, PreparedTxDma};

pub fn prepared(
    dma: &dyn PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>,
) -> DmaRef<WifiTxDmaDescriptor> {
    let head = dma.descriptor_head();
    #[cfg(ui_reject)]
    let _: u32 = head; // ui-error: E0308
    head
}

pub fn hardware_owned(
    dma: &dyn HardwareOwnedTxDma<DmaRef<WifiTxDmaDescriptor>>,
) -> DmaRef<WifiTxDmaDescriptor> {
    let head = dma.descriptor_head();
    #[cfg(ui_reject)]
    let _: u32 = head; // ui-error: E0308
    head
}
