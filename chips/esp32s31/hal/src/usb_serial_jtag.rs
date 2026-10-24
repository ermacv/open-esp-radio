//! The USB Serial/JTAG console on this chip: the early writer before the
//! console's driver exists, and the parts the driver is built from.
//!
//! The block's holder is this chip's one
//! [`Ownership`](oer_espressif_usb_serial_jtag::ownership::Ownership): the
//! early writer builds the block's owner for each record it holds the block
//! for, until the esp-hal adapter retires it ([`retire_early_writer`]) and
//! takes the block for the driver; then every early record goes through the
//! driver ([`oer_espressif_usb_serial_jtag::Endpoint`]), whose parts
//! ([`console_parts`]) check that the driver still holds the block.

use oer_diagnostics::Note;
use oer_esp32s31_pac::platform::{UsbEndpointRegisters, UsbEvents, UsbInterruptRegisters};
pub use oer_esp32s31_pac::platform::{
    UsbPadRegisters as UsbPads, UsbSerialJtagHandoff, UsbSerialJtagPlacement,
    UsbSerialJtagRegisters,
};
pub use oer_espressif_usb_serial_jtag::ownership::RetireError;
use oer_espressif_usb_serial_jtag::ownership::{Guarded, Owner, Ownership};
use oer_espressif_usb_serial_jtag::{EndpointRegisters, Events, InterruptRegisters};
use oer_mmio::Placement;

#[cfg(test)]
mod tests;

/// The holder of the chip's block.
static OWNERSHIP: Ownership = Ownership::new();

/// The chip's console, whose [`Ownership`] the driver's parts check.
pub struct ChipConsole;

impl Owner for ChipConsole {
    fn ownership() -> &'static Ownership {
        &OWNERSHIP
    }
}

/// The driver's task registers: they make no access once the fatal path
/// took the block.
pub type ConsoleEndpoint = Guarded<ChipConsole, UsbEndpoint>;

/// The driver's interrupt handler registers: they read and acknowledge
/// nothing once the fatal path took the block.
pub type ConsoleInterrupt = Guarded<ChipConsole, UsbInterrupt>;

/// The task's registers of the console.
pub struct UsbEndpoint<P: Placement = UsbSerialJtagPlacement>(UsbEndpointRegisters<P>);

/// The interrupt handler's registers of the console.
pub struct UsbInterrupt<P: Placement = UsbSerialJtagPlacement>(UsbInterruptRegisters<P>);

/// The console driver's endpoint, interrupt and pads from the block's
/// owner the esp-hal adapter took.
pub fn console_parts(
    registers: UsbSerialJtagRegisters,
) -> (ConsoleEndpoint, ConsoleInterrupt, UsbPads) {
    let (endpoint, interrupt, pads) = parts(registers);
    (Guarded::new(endpoint), Guarded::new(interrupt), pads)
}

/// The endpoint, interrupt and pads of a block's owner.
fn parts<P: Placement>(
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
/// record or the fatal note.
#[allow(
    unsafe_code,
    reason = "the early and fatal writers' capture of the block they hold"
)]
fn capture() -> UsbEndpoint {
    // SAFETY: `OWNERSHIP` calls this only after its compare-and-swap made
    // the caller the block's holder. For an early record it moved the block
    // from free and frees it only after this owner is dropped; neither the
    // esp-hal adapter's handoff (`retire_early_writer`) nor another record
    // takes a block that is not free. For the fatal note it moved the block
    // from free, when no driver and no record holds it, or from the driver
    // only for `fatal_note` on the driver's hart under a critical section on
    // a path that never returns: the driver's task and interrupt handler,
    // suspended there, never run again, and their guarded parts make no
    // access once the block is the fatal path's. Afterwards no one takes the
    // block. So no other owner accesses the block while this one lives.
    let handoff = unsafe { UsbSerialJtagHandoff::assume_exclusive() };
    parts(UsbSerialJtagRegisters::from_handoff(handoff)).0
}
/// Writes `note` as one `OER-DIAG:01` line through the early writer, or
/// drops and counts it while another record holds the block. Returns
/// `false` once the driver or the fatal path took the block: the driver
/// writes notes after [`retire_early_writer`].
pub fn early_note(note: &Note<'_>) -> bool {
    OWNERSHIP.write(capture, |writer| writer.note(note))
}

/// Writes `bytes` as they are, for the bootstrap's `OER_BOOT` lines, or
/// drops and counts them while another record holds the block. Returns
/// `false` once the driver or the fatal path took the block.
pub fn early_text(bytes: &[u8]) -> bool {
    OWNERSHIP.write(capture, |writer| writer.text(bytes))
}

/// Bytes the early writer dropped since the last call, for the driver's
/// owner to report: a loss is never silent.
pub fn take_early_lost() -> u32 {
    OWNERSHIP.take_lost()
}

/// Proof that the console's driver holds the block: the early writer
/// builds no owner of it any more.
#[must_use = "the driver's handoff relies on the retired early writer"]
pub struct EarlyWriterRetired(());

/// Give the block to the console's driver for good, if no one holds it: no
/// early record takes the block after this call. Never waits.
///
/// # Errors
/// An early record holds the block now ([`RetireError::Busy`]), or the
/// driver or the fatal path took it before ([`RetireError::Taken`]).
pub fn retire_early_writer() -> Result<EarlyWriterRetired, RetireError> {
    OWNERSHIP.retire().map(|()| EarlyWriterRetired(()))
}

/// The hart the console's driver runs on: the boot hart's executor.
#[cfg(target_arch = "riscv32")]
const DRIVER_HART: usize = 0;

/// Writes `note` on a path that never returns, then `halt`s: the fatal
/// takeover of the block, which never waits. It takes the block if no one
/// holds it, or from the console's driver when called on the driver's hart:
/// the critical section masks interrupts there, so neither the driver's
/// task nor its interrupt handler runs while this writes, and neither runs
/// again, since this never returns and holds the section for good. The
/// driver's guarded parts make no access afterwards. Called from another
/// hart, or while an early record or an earlier fatal note holds the block,
/// the note is not written and its bytes are counted instead; the previous
/// boot's record is kept elsewhere. A packet the driver had half written
/// may be lost.
#[cfg(target_arch = "riscv32")]
#[allow(unsafe_code, reason = "the read of the hart's identity")]
pub fn fatal_note(
    note: &Note<'_>,
    _driver_masked: critical_section::CriticalSection<'_>,
    halt: impl FnOnce() -> !,
) -> ! {
    let hart: usize;
    // SAFETY: reading `mhartid` touches no memory.
    unsafe {
        core::arch::asm!("csrr {hart}, mhartid", hart = out(reg) hart, options(nomem, nostack));
    }
    let _written = OWNERSHIP.write_fatal(hart == DRIVER_HART, capture, note);
    halt()
}
