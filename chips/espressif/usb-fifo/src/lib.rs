#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! The synchronous writer of the USB Serial/JTAG transmit FIFO, for the time
//! before a USB driver owns the peripheral: the bootstrap, a runtime's early
//! boot and the report of the previous boot's fatal record. It writes bytes
//! while the FIFO has room, drops the rest and counts what it dropped
//! ([`oer_diagnostics::FifoWriter`]). It waits for room within a budget per
//! record, as there is no executor to yield to; [`fatal_note`] never waits.
//!
//! `USB_DEVICE` has one owner at a time. The writer owns it from reset until
//! [`hand_over`] takes the peripheral's token on its way to the driver; after
//! that, every write here is refused and output goes through the driver's
//! owner. The writer reaches the registers through esp-hal's typed
//! accessors of the block, which esp-hal owns at runtime.

use core::sync::atomic::{AtomicBool, Ordering};

use esp_hal::peripherals::USB_DEVICE;
use oer_diagnostics::{Fifo, FifoWriter, Lost, Note, Patience};

/// What the early writer may wait for room in the 64-byte FIFO, per record,
/// in polls of the room flag: the host takes a packet in about a millisecond,
/// and a poll is a register read of well under a microsecond, so the budget is
/// some milliseconds up to tens of them at the early boot's slow clock. Before
/// an executor exists there is nothing to yield to.
const EARLY_PATIENCE: Patience = Patience::polls(100_000);

static LOST: Lost = Lost::new();
/// Whether a driver has taken the peripheral from the early writer.
static DRIVER_OWNS: AtomicBool = AtomicBool::new(false);

/// The transmit FIFO through the block's typed accessors.
struct Endpoint(USB_DEVICE<'static>);

impl Fifo for Endpoint {
    fn has_room(&self) -> bool {
        self.0
            .register_block()
            .ep1_conf()
            .read()
            .serial_in_ep_data_free()
            .bit_is_set()
    }

    #[allow(unsafe_code, reason = "the PAC's raw 8-bit data field")]
    fn push(&mut self, byte: u8) {
        let ep1 = self.0.register_block().ep1();
        // SAFETY: `rdwr_byte` takes any 8-bit value; the caller saw room.
        ep1.write(|w| unsafe { w.rdwr_byte().bits(byte) });
    }

    fn flush(&mut self) {
        self.0
            .register_block()
            .ep1_conf()
            .modify(|_, w| w.wr_done().set_bit());
    }
}

/// Runs `write` with a writer of `patience` over a fresh capture of the
/// peripheral.
///
/// # Safety
/// Nothing else may use `USB_DEVICE` while `write` runs.
#[allow(unsafe_code, reason = "the early boot's capture of the peripheral")]
unsafe fn write_captured(patience: Patience, write: impl FnOnce(&mut FifoWriter<'_, Endpoint>)) {
    // SAFETY: the caller's contract.
    let token = unsafe { USB_DEVICE::steal() };
    write(&mut FifoWriter::new(Endpoint(token), &LOST, patience));
}

/// Runs `write` with the early writer, unless a driver owns the peripheral.
#[allow(unsafe_code, reason = "the early boot's one capture of the peripheral")]
fn with_writer(write: impl FnOnce(&mut FifoWriter<'_, Endpoint>)) -> bool {
    if DRIVER_OWNS.load(Ordering::Acquire) {
        return false;
    }
    // SAFETY: until `hand_over`, nothing else holds `USB_DEVICE`: the early
    // boot runs on one hart, no driver exists, and the token only goes to a
    // driver through `hand_over`, which stops this writer first.
    unsafe { write_captured(EARLY_PATIENCE, write) };
    true
}

/// Writes `note` as one `OER-DIAG:01` line. Returns whether the early writer
/// owns the peripheral; a driver's owner writes notes after [`hand_over`].
pub fn note(note: &Note<'_>) -> bool {
    with_writer(|writer| writer.note(note))
}

/// Writes `bytes` as they are, for the bootstrap's `OER_BOOT` lines. Returns
/// whether the early writer owns the peripheral.
pub fn text(bytes: &[u8]) -> bool {
    with_writer(|writer| writer.text(bytes))
}

/// Gives the peripheral to its driver: `token` is the one the driver is built
/// from. No write of this crate succeeds after this call.
pub fn hand_over(token: USB_DEVICE<'static>) -> USB_DEVICE<'static> {
    DRIVER_OWNS.store(true, Ordering::Release);
    token
}

/// Bytes dropped by the early writer since the last call, for the driver's
/// owner to report once it can: a loss is never silent.
pub fn take_lost() -> u32 {
    LOST.take()
}

/// A FIFO that never has room: a note written to it is only counted.
struct Closed;

impl Fifo for Closed {
    fn has_room(&self) -> bool {
        false
    }

    fn push(&mut self, _: u8) {}

    fn flush(&mut self) {}
}

/// Writes `note` on a path that never returns, taking the peripheral from the
/// driver: the one named capture. The critical section proves that
/// interrupts are masked on the calling hart, and the driver runs on the
/// boot hart (`ProCpu`) alone, so on that hart it cannot write at the same
/// time. Called from any other hart the driver may still run, so the note is
/// not written and its bytes are counted in the loss instead. A byte the
/// driver had half written may be lost; the previous boot's record is kept
/// elsewhere.
#[allow(
    unsafe_code,
    reason = "the named capture of the peripheral for a fatal path"
)]
pub fn fatal_note(note: &Note<'_>, _transport_masked: critical_section::CriticalSection<'_>) {
    if esp_hal::system::Cpu::current() != esp_hal::system::Cpu::ProCpu {
        FifoWriter::new(Closed, &LOST, Patience::NONE).note(note);
        return;
    }
    // SAFETY: the driver cannot execute while the caller's critical section
    // holds its hart, which is the driver's, and the caller does not return
    // to it.
    unsafe { write_captured(Patience::NONE, |writer| writer.note(note)) };
}
