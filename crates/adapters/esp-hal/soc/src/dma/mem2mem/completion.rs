//! Channel-zero interrupt wakeup and race-closed Future polling.

use super::{
    registers::{disable_channel_interrupts, enable_channel_interrupts, terminal_status},
    transfer::{AxiGdmaMem2MemReport, AxiGdmaMem2MemTransferError, AxiGdmaMem2MemTransferOwner},
};
use atomic_waker::AtomicWaker;
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

oer_memory::zeroed_static! {
    /// Channel zero's waker. `AtomicWaker` has no guaranteed zero
    /// representation, so the zeroed cell holds it uninitialized until the
    /// first poll writes it; the interrupt, enabled only after that poll
    /// registered a waker, reads it without waiting.
    static CHANNEL0_WAKER: oer_memory::zeroed::ZeroedOnce<AtomicWaker> =
        zeroed in ".flash.critical.bss.axi_gdma_mem2mem";
}

/// The interrupt-table handler of both channel-0 sources
/// (`AXI_PDMA_IN_CH0`, `AXI_PDMA_OUT_CH0`).
#[inline(never)]
#[unsafe(link_section = ".rwtext.axi_gdma_mem2mem")]
pub fn channel0_interrupt() {
    disable_channel_interrupts();
    if let Some(waker) = CHANNEL0_WAKER.get() {
        waker.wake();
    }
}

impl Future for AxiGdmaMem2MemTransferOwner<'_, '_, '_> {
    type Output = Result<AxiGdmaMem2MemReport, AxiGdmaMem2MemTransferError>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        assert!(this.active, "AXI-GDMA transfer polled after completion");

        if let Some(status) = terminal_status() {
            return Poll::Ready(this.finish(status));
        }

        CHANNEL0_WAKER
            .get_or_init(AtomicWaker::new)
            .register(context.waker());
        enable_channel_interrupts();

        // Re-check after publishing the waker and enabling the peripheral
        // sources. This closes both completion-before-registration and
        // completion-between-registration-and-enable races.
        if let Some(status) = terminal_status() {
            disable_channel_interrupts();
            Poll::Ready(this.finish(status))
        } else {
            Poll::Pending
        }
    }
}
