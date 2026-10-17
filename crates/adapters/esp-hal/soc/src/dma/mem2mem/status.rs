//! Typed terminal interrupt state of the channel-zero memory copy.
//!
//! A transfer fails on the error EOF and descriptor errors only, as in both
//! vendor drivers: ESP-IDF 4d59230d `esp_driver_dma/src/gdma.c` enables and
//! dispatches SUC_EOF, ERR_EOF and DESC_ERROR, and esp-hal 6336b45
//! `esp-hal/src/dma/mod.rs` fails an RX transfer on ErrorEof,
//! DescriptorError and DescriptorEmpty. Neither treats the channel's FIFO
//! overflow or underflow events (`esp_hal_dma/esp32s31/include/hal/gdma_ll.h`)
//! as a failure; they are reported for observation.

/// Channel-zero input (RX) interrupt sources observed at completion.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AxiGdmaMem2MemRxStatus {
    pub done: bool,
    pub success_eof: bool,
    pub error_eof: bool,
    pub descriptor_error: bool,
    pub descriptor_empty: bool,
    pub fifo_overflow: bool,
    pub fifo_underflow: bool,
}

impl AxiGdmaMem2MemRxStatus {
    /// Whether the input channel reported a failed transfer.
    pub const fn failed(self) -> bool {
        self.error_eof || self.descriptor_error || self.descriptor_empty
    }
}

/// Channel-zero output (TX) interrupt sources observed at completion.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AxiGdmaMem2MemTxStatus {
    pub done: bool,
    pub eof: bool,
    pub descriptor_error: bool,
    pub total_eof: bool,
    pub fifo_overflow: bool,
    pub fifo_underflow: bool,
}

impl AxiGdmaMem2MemTxStatus {
    /// Whether the output channel reported a failed transfer.
    pub const fn failed(self) -> bool {
        self.descriptor_error
    }
}

/// Raw interrupt state of both channel directions at a terminal edge.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AxiGdmaMem2MemStatus {
    pub rx: AxiGdmaMem2MemRxStatus,
    pub tx: AxiGdmaMem2MemTxStatus,
}

impl AxiGdmaMem2MemStatus {
    /// Whether either direction reported a failed transfer.
    pub const fn failed(self) -> bool {
        self.rx.failed() || self.tx.failed()
    }

    /// Whether a transfer failed or both directions reached their end.
    pub const fn terminal(self) -> bool {
        self.failed() || (self.rx.success_eof && self.tx.total_eof)
    }
}

#[cfg(test)]
mod tests;
