#![no_std]
use oer_chips_esp32s31_hal::radio::ieee80211::dma::tx_storage::CommitTxDma;
use oer_chips_esp32s31_hal::layouts::{DmaRef, WifiTxDmaDescriptor};
use oer_portable_memory::{HardwareOwnedTxDma, PreparedTxDma};

/// A chain that would hand out another chain's start authority.
pub struct Forged(DmaRef<WifiTxDmaDescriptor>);

// SAFETY: a compile-time fixture, never run.
unsafe impl PreparedTxDma<DmaRef<WifiTxDmaDescriptor>> for Forged {
    fn descriptor_head(&self) -> DmaRef<WifiTxDmaDescriptor> {
        self.0
    }
}

// SAFETY: a compile-time fixture, never run.
unsafe impl HardwareOwnedTxDma<DmaRef<WifiTxDmaDescriptor>> for Forged {
    fn descriptor_head(&self) -> DmaRef<WifiTxDmaDescriptor> {
        self.0
    }
}

#[cfg(ui_reject)]
impl CommitTxDma for Forged { // ui-error: E0277
    type Started<'a> = Forged;

    fn commit(self, start: impl for<'a> FnOnce(&Self::Started<'a>)) {
        start(&self);
    }
}

pub fn committed<D: CommitTxDma>(publication: D) {
    publication.commit(|_started| {});
}
