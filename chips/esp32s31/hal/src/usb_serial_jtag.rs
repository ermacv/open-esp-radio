//! The USB Serial/JTAG console on this chip: the early writer before the
//! console's driver exists, and the parts the driver is built from.
//!
//! The early writer builds the block's owner for each record it writes,
//! until the esp-hal adapter retires it ([`retire_early_writer`]) and takes
//! the block for the driver; then every early write is refused and output
//! goes through the driver ([`oer_espressif_usb_serial_jtag::Endpoint`]).

use oer_diagnostics::Note;
use oer_esp32s31_pac::platform::{UsbEndpointRegisters, UsbEvents, UsbInterruptRegisters};
pub use oer_esp32s31_pac::platform::{
    UsbPadRegisters as UsbPads, UsbSerialJtagHandoff, UsbSerialJtagPlacement,
    UsbSerialJtagRegisters,
};
use oer_espressif_usb_serial_jtag::{EndpointRegisters, Events, InterruptRegisters, early::Early};
use oer_mmio::Placement;

#[cfg(test)]
mod tests;

/// The early writer of the chip.
static EARLY: Early = Early::new();

/// The task's registers of the console.
pub struct UsbEndpoint<P: Placement = UsbSerialJtagPlacement>(UsbEndpointRegisters<P>);

/// The interrupt handler's registers of the console.
pub struct UsbInterrupt<P: Placement = UsbSerialJtagPlacement>(UsbInterruptRegisters<P>);

/// The console's endpoint, interrupt and pads from the block's owner.
pub fn console_parts<P: Placement>(
    registers: UsbSerialJtagRegisters<P>,
) -> (UsbEndpoint<P>, UsbInterrupt<P>, UsbPads<P>) {
    let (endpoint, interrupt, pads) = registers.into_parts();
    (UsbEndpoint(endpoint), UsbInterrupt(interrupt), pads)
}

impl<P: Placement> EndpointRegisters for UsbEndpoint<P> {
    fn has_room(&self) -> bool {
        self.0.has_room()
    }

    fn push(&mut self, byte: u8) {
        self.0.push(byte);
    }

    fn send(&mut self) {
        self.0.send();
    }

    fn has_data(&self) -> bool {
        self.0.has_data()
    }

    fn pop(&mut self) -> u8 {
        self.0.pop()
    }

    fn enable_events(&mut self) {
        self.0.enable_events();
    }
}

impl<P: Placement> InterruptRegisters for UsbInterrupt<P> {
    fn take_events(&mut self) -> Events {
        let UsbEvents {
            in_empty,
            out_received,
        } = self.0.take_events();
        Events {
            in_empty,
            out_received,
        }
    }
}

/// The endpoint registers of a fresh owner of the block, for one early
/// record.
#[allow(
    unsafe_code,
    reason = "the early writer's capture of the block for one record"
)]
fn capture() -> UsbEndpoint {
    // SAFETY: `EARLY` calls this only until `retire_early_writer`, and the
    // esp-hal adapter retires it before it makes the driver's owner; the
    // early writer runs on the boot hart before any executor or interrupt
    // handler uses the block, one record at a time, and drops this owner
    // when the record is written.
    let handoff = unsafe { UsbSerialJtagHandoff::assume_exclusive() };
    console_parts(UsbSerialJtagRegisters::from_handoff(handoff)).0
}

/// Writes `note` as one `OER-DIAG:01` line through the early writer.
/// Returns whether it still owns the block; the driver writes notes after
/// [`retire_early_writer`].
pub fn early_note(note: &Note<'_>) -> bool {
    EARLY.write(capture, |writer| writer.note(note))
}

/// Writes `bytes` as they are, for the bootstrap's `OER_BOOT` lines.
/// Returns whether the early writer still owns the block.
pub fn early_text(bytes: &[u8]) -> bool {
    EARLY.write(capture, |writer| writer.text(bytes))
}

/// Bytes the early writer dropped since the last call, for the driver's
/// owner to report: a loss is never silent.
pub fn take_early_lost() -> u32 {
    EARLY.take_lost()
}

/// Proof that the early writer is retired: it builds no owner of the block
/// any more.
#[must_use = "the driver's handoff relies on the retired early writer"]
pub struct EarlyWriterRetired(());

/// Retire the early writer for good, before the console's driver takes the
/// block: no early write succeeds after this call.
pub fn retire_early_writer() -> EarlyWriterRetired {
    EARLY.retire();
    EarlyWriterRetired(())
}

/// The hart the console's driver runs on: the boot hart's executor.
#[cfg(target_arch = "riscv32")]
const DRIVER_HART: usize = 0;

/// Writes `note` on a path that never returns, taking the block from the
/// console's driver: the one named capture, which never waits. The
/// critical section masks interrupts on the calling hart; on the driver's
/// hart the driver cannot run while this writes, and the caller does not
/// return to it. Called from another hart, the driver may still run, so the
/// note is not written and its bytes are counted instead; the previous
/// boot's record is kept elsewhere. A byte the driver had half written may
/// be lost.
#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the named capture of the block for a fatal path"
)]
pub fn fatal_note(note: &Note<'_>, _driver_masked: critical_section::CriticalSection<'_>) {
    let hart: usize;
    // SAFETY: reading `mhartid` touches no memory.
    unsafe {
        core::arch::asm!("csrr {hart}, mhartid", hart = out(reg) hart, options(nomem, nostack));
    }
    if hart != DRIVER_HART {
        EARLY.write_fatal::<UsbEndpoint>(None, note);
        return;
    }
    // SAFETY: the caller's critical section holds the driver's hart, so the
    // driver does not execute while the owner made here lives; it is dropped
    // before this returns, and the caller never resumes the driver. The early
    // writer runs only before the driver exists, on this hart.
    let handoff = unsafe { UsbSerialJtagHandoff::assume_exclusive() };
    let owner = console_parts(UsbSerialJtagRegisters::from_handoff(handoff)).0;
    EARLY.write_fatal(Some(owner), note);
}
