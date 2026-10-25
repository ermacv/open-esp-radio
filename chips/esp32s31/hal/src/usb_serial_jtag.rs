//! The USB Serial/JTAG console on this chip: the early writer before the
//! console's driver exists, the parts the driver is built from, and the
//! fatal note.
//!
//! The block's holder is this chip's one
//! [`Ownership`](oer_espressif_usb_serial_jtag::ownership::Ownership): the
//! early writer builds the block's owner for each record it holds the block
//! for, until the esp-hal adapter retires it ([`retire_early_writer`]) and
//! takes the block for the driver; then every early record goes through the
//! driver ([`oer_espressif_usb_serial_jtag::Endpoint`]), which holds the
//! block for good. The fatal note ([`fatal_note`]) takes only a free block.
//!
//! The driver's endpoint and pads stay on the hart that took them: neither
//! is `Send`, so no other hart's task or handler reaches them
//! (`tests/ui/usb_console_parts_stay_on_their_hart.rs`).
//!
//! The interrupt's part moves once, into the handler's static, which needs
//! it `Send`; the image's interrupt table routes that handler to the
//! driver's hart.

use core::marker::PhantomData;

use oer_diagnostics::Note;
pub use oer_esp32s31_pac::platform::{
    UsbSerialJtagHandoff, UsbSerialJtagPlacement, UsbSerialJtagRegisters,
};
use oer_esp32s31_pac::{
    usb_serial_jtag::{UsbSerialJtagIrq, UsbSerialJtagTask},
    usb_serial_jtag_pads::UsbSerialJtagPads,
};
use oer_espressif_usb_serial_jtag::ownership::Ownership;
pub use oer_espressif_usb_serial_jtag::ownership::RetireError;
use oer_espressif_usb_serial_jtag::{EndpointRegisters, Events, InterruptRegisters};
use oer_mmio::Placement;

#[cfg(test)]
mod tests;

/// The holder of the chip's block.
static OWNERSHIP: Ownership = Ownership::new();

/// The driver's task registers.
pub type ConsoleEndpoint = UsbEndpoint;

/// The driver's interrupt handler registers.
pub type ConsoleInterrupt = UsbInterrupt;

/// Keeps a part on the hart that took it: not `Send`.
type ThisHart = PhantomData<*const ()>;

/// The task's registers of the console, which stay on their hart.
pub struct UsbEndpoint<P: Placement = UsbSerialJtagPlacement>(UsbSerialJtagTask<P>, ThisHart);

/// The interrupt handler's registers of the console.
pub struct UsbInterrupt<P: Placement = UsbSerialJtagPlacement>(UsbSerialJtagIrq<P>);

/// The console's pads, which stay on their hart.
pub struct UsbPads<P: Placement = UsbSerialJtagPlacement>(UsbSerialJtagPads<P>, ThisHart);

impl<P: Placement> UsbPads<P> {
    /// Takes the USB Serial/JTAG off its pads and drops its pull-ups: the
    /// host sees the device leave USB.
    pub fn disconnect(self) {
        let mut pads = self.0;
        pads.modify_conf0(|pads| {
            pads.set_pad_pull_override(true)
                .set_dp_pullup(false)
                .set_dm_pullup(false)
                .set_usb_pad_enable(false);
        });
    }
}

/// The console driver's endpoint, interrupt and pads from the block's
/// owner the esp-hal adapter took.
pub fn console_parts(
    registers: UsbSerialJtagRegisters,
) -> (ConsoleEndpoint, ConsoleInterrupt, UsbPads) {
    parts(registers)
}

/// The endpoint, interrupt and pads of a block's owner.
fn parts<P: Placement>(
    registers: UsbSerialJtagRegisters<P>,
) -> (UsbEndpoint<P>, UsbInterrupt<P>, UsbPads<P>) {
    let (endpoint, interrupt) = registers.endpoint.split();
    let pads = registers.pads;
    (
        UsbEndpoint(endpoint, PhantomData),
        UsbInterrupt(interrupt),
        UsbPads(pads, PhantomData),
    )
}

impl<P: Placement> EndpointRegisters for UsbEndpoint<P> {
    fn has_room(&self) -> bool {
        self.0.read_ep1_conf().serial_in_ep_data_free()
    }

    fn push(&mut self, byte: u8) {
        self.0.push_ep1(byte);
    }

    fn send(&mut self) {
        self.0.trigger_ep1_conf_wr_done();
    }

    fn has_data(&self) -> bool {
        self.0.read_ep1_conf().serial_out_ep_data_avail()
    }

    fn pop(&mut self) -> u8 {
        self.0.pop_ep1()
    }

    fn enable_events(&mut self) {
        self.0.modify_int_ena(|enables| {
            enables
                .set_serial_in_empty_int_ena(true)
                .set_serial_out_recv_pkt_int_ena(true);
        });
    }
}

impl<P: Placement> InterruptRegisters for UsbInterrupt<P> {
    fn take_events(&mut self) -> Events {
        let status = self.0.sample_usb_console_interrupt();
        let events = Events {
            in_empty: status.serial_in_empty_int_st(),
            out_received: status.serial_out_recv_pkt_int_st(),
        };
        if events.in_empty || events.out_received {
            self.0.acknowledge_usb_console_interrupt(status);
        }
        events
    }
}

/// The endpoint registers of a fresh owner of the block, for one early
/// record or the fatal note.
#[allow(
    unsafe_code,
    reason = "the early and fatal writers' capture of the block they hold"
)]
fn capture() -> UsbEndpoint {
    // SAFETY: `OWNERSHIP` calls this only after its compare-and-swap moved
    // the block from free to the caller: to one early record, which frees
    // it only after this owner is dropped, or to the fatal note, for good.
    // Neither the esp-hal adapter's handoff (`retire_early_writer`), nor
    // another record, nor another fatal note takes a block that is not
    // free, and nothing takes it from its holder. So no other owner of the
    // block exists while this one lives.
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

/// Where a fatal note went.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "a held note is kept for the next boot by the caller"]
pub enum FatalNote {
    /// The block was free: the note went to the host, and the block stays
    /// the fatal path's for good.
    Written,
    /// The console's driver, an early record or an earlier fatal note
    /// holds the block: nothing touched it, and the caller keeps the note
    /// for the next boot.
    Held,
}

/// Writes `note` without waiting, if no one holds the block; the block then
/// stays the fatal path's for good. It never takes the block from its
/// holder, so the console's driver, on any hart, keeps the only owner of
/// the block. Takes no lock and masks nothing.
pub fn fatal_note(note: &Note<'_>) -> FatalNote {
    if OWNERSHIP.write_fatal(capture, note) {
        FatalNote::Written
    } else {
        FatalNote::Held
    }
}

#[cfg(test)]
mod register_tests;
