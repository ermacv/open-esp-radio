//! The USB Serial/JTAG console on this chip: the early writer before the
//! console's driver exists, the parts the driver is built from, and the
//! fatal note.
//!
//! The reset root installs the console's parts once ([`install`]) in this
//! chip's one [`Ownership`](oer_espressif_usb_serial_jtag::ownership::Ownership):
//! each early record borrows the endpoint while it writes, until the
//! console's driver takes the parts for good ([`retire_early_writer`]);
//! then every early record goes through the driver
//! ([`oer_espressif_usb_serial_jtag::Endpoint`]). The fatal note
//! ([`fatal_note`]) takes only a free block, for good. A record before the
//! root installed the parts is counted as lost.
//!
//! The driver's endpoint and pads stay on the hart that took them: neither
//! is `Send`, so no other hart's task or handler reaches them
//! (`tests/ui/usb_console_parts_stay_on_their_hart.rs`).
//!
//! The interrupt's part moves once, into the handler's storage, which needs
//! it `Send`; the image's interrupt table routes that handler to the
//! driver's hart. Enabling events takes that part's `Bound` or `Silent`
//! proof, including when the task registers are used directly.

use core::marker::PhantomData;

use oer_diagnostics::Note;
use oer_esp32s31_pac::{
    instances,
    ownership::UsbSerialJtagPeripherals,
    usb_serial_jtag::{UsbSerialJtagIrq, UsbSerialJtagPads, UsbSerialJtagTask},
};
pub use oer_espressif_usb_serial_jtag::ownership::RetireError;
use oer_espressif_usb_serial_jtag::ownership::{Console, Ownership};
use oer_espressif_usb_serial_jtag::{EndpointRegisters, Events, InterruptRegisters};
use oer_mmio::{Fixed, InterruptSource, Placement};

#[cfg(test)]
mod tests;

/// Where the device's USB Serial/JTAG block lies.
pub type UsbSerialJtagPlacement = Fixed<{ instances::usb_device::BASE }>;

/// The console's parts as the root installs them: they may reach any hart's
/// early record or fatal note, so they are `Send`; the driver's parts made
/// from them stay on its hart.
pub struct ConsoleRegisters<P: Placement = UsbSerialJtagPlacement> {
    task: Endpoint<P>,
    irq: UsbSerialJtagIrq<P>,
    pads: UsbSerialJtagPads<P>,
}

/// The task's registers of the block, which the early and fatal writers
/// borrow.
pub struct Endpoint<P: Placement = UsbSerialJtagPlacement>(UsbSerialJtagTask<P>);

impl<P: Placement> Console for ConsoleRegisters<P> {
    type Endpoint = Endpoint<P>;

    fn endpoint(&mut self) -> &mut Endpoint<P> {
        &mut self.task
    }
}

impl ConsoleRegisters {
    /// The root's console partition.
    pub(crate) fn new(peripherals: UsbSerialJtagPeripherals) -> Self {
        let UsbSerialJtagPeripherals {
            usb_serial_jtag_task,
            usb_serial_jtag_irq,
            usb_serial_jtag_pads,
        } = peripherals;
        Self {
            task: Endpoint(usb_serial_jtag_task),
            irq: usb_serial_jtag_irq,
            pads: usb_serial_jtag_pads,
        }
    }
}

/// The holder of the chip's block.
static OWNERSHIP: Ownership<ConsoleRegisters> = Ownership::new();

/// The driver's task registers.
pub type ConsoleEndpoint = UsbEndpoint;

/// The driver's interrupt handler registers.
pub type ConsoleInterrupt = UsbInterrupt;

/// Keeps a part on the hart that took it: not `Send`.
type ThisHart = PhantomData<*const ()>;

/// The task's registers of the console, which stay on their hart.
pub struct UsbEndpoint<P: Placement = UsbSerialJtagPlacement>(Endpoint<P>, ThisHart);

/// The interrupt handler's registers of the console; the part names the
/// console's interrupt source.
pub struct UsbInterrupt<P: Placement = UsbSerialJtagPlacement>(UsbSerialJtagIrq<P>);

impl<P: Placement> InterruptSource for UsbInterrupt<P> {
    const SOURCE: u16 = <UsbSerialJtagIrq<P> as InterruptSource>::SOURCE;
}

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

/// The driver's endpoint, interrupt and pads of the console's parts.
fn parts<P: Placement>(
    registers: ConsoleRegisters<P>,
) -> (UsbEndpoint<P>, UsbInterrupt<P>, UsbPads<P>) {
    let ConsoleRegisters { task, irq, pads } = registers;
    (
        UsbEndpoint(task, PhantomData),
        UsbInterrupt(irq),
        UsbPads(pads, PhantomData),
    )
}

impl<P: Placement> EndpointRegisters for Endpoint<P> {
    type Interrupt = UsbInterrupt<P>;

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

    fn enable_events(
        &mut self,
        _authority: &impl oer_interrupt_table::EventAuthority<Self::Interrupt>,
    ) {
        self.0.modify_int_ena(|enables| {
            enables
                .set_serial_in_empty_int_ena(true)
                .set_serial_out_recv_pkt_int_ena(true);
        });
    }
}

impl<P: Placement> EndpointRegisters for UsbEndpoint<P> {
    type Interrupt = UsbInterrupt<P>;

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

    fn enable_events(
        &mut self,
        authority: &impl oer_interrupt_table::EventAuthority<Self::Interrupt>,
    ) {
        self.0.enable_events(authority);
    }
}

impl<P: Placement> InterruptRegisters for UsbInterrupt<P> {
    /// Sample the console's two events and acknowledge exactly those.
    ///
    /// SOURCE: ESP-IDF 4d80c1b2 `usb_serial_jtag_ll.h`
    /// (`usb_serial_jtag_ll_get_intsts_mask`, `usb_serial_jtag_ll_clr_intsts_mask`).
    fn take_events(&mut self) -> Events {
        let status = self.0.sample_usb_serial_jtag_interrupt();
        let events = Events {
            in_empty: status.serial_in_empty_int_st(),
            out_received: status.serial_out_recv_pkt_int_st(),
        };
        if !status.is_clear() {
            self.0.acknowledge_usb_serial_jtag_interrupt(status);
        }
        events
    }
}

/// The console's parts were installed before.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Installed;

/// Install the console's parts for the early writer, the fatal note and
/// the driver, once, from the reset root.
pub(crate) fn install(registers: ConsoleRegisters) -> Result<(), Installed> {
    install_in(&OWNERSHIP, registers)
}

fn install_in<T: Console>(ownership: &Ownership<T>, registers: T) -> Result<(), Installed> {
    ownership.install(registers).map_err(|_| Installed)
}

/// Writes `note` as one `OER-DIAG:01` line through the early writer, or
/// drops and counts it while another record holds the block. Returns
/// `false` once the driver or the fatal path took the block: the driver
/// writes notes after [`retire_early_writer`].
pub fn early_note(note: &Note<'_>) -> bool {
    OWNERSHIP.write(|writer| writer.note(note))
}

/// Writes `bytes` as they are, for the bootstrap's `OER_BOOT` lines, or
/// drops and counts them while another record holds the block. Returns
/// `false` once the driver or the fatal path took the block.
pub fn early_text(bytes: &[u8]) -> bool {
    OWNERSHIP.write(|writer| writer.text(bytes))
}

/// Bytes the early writer dropped since the last call, for the driver's
/// owner to report: a loss is never silent.
pub fn take_early_lost() -> u32 {
    OWNERSHIP.take_lost()
}

/// Give the block to the console's driver for good, if no one holds it: the
/// driver's endpoint, interrupt and pads; no early record takes the block
/// after this call. Never waits.
///
/// # Errors
/// No parts were installed ([`RetireError::Empty`]), an early record holds
/// the block now ([`RetireError::Busy`]), or the driver or the fatal path
/// took it before ([`RetireError::Taken`]).
pub fn retire_early_writer() -> Result<(ConsoleEndpoint, ConsoleInterrupt, UsbPads), RetireError> {
    OWNERSHIP.retire().map(parts)
}

/// Where a fatal note went.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "a held note is kept for the next boot by the caller"]
pub enum FatalNote {
    /// The block was free: the note went to the host, and the block stays
    /// the fatal path's for good.
    Written,
    /// The console's driver, an early record or an earlier fatal note
    /// holds the block, or the root installed no parts: nothing touched
    /// it, and the caller keeps the note for the next boot.
    Held,
}

/// Writes `note` without waiting, if no one holds the block; the block then
/// stays the fatal path's for good. It never takes the block from its
/// holder, so the console's driver, on any hart, keeps the only owner of
/// the block. Takes no lock and masks nothing.
pub fn fatal_note(note: &Note<'_>) -> FatalNote {
    if OWNERSHIP.write_fatal(note) {
        FatalNote::Written
    } else {
        FatalNote::Held
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> ConsoleRegisters<oer_mmio::Host<'a>> {
    /// The console's parts over host memory from the block's base: one
    /// memory for the task's and the handler's divided words, one for the
    /// pads.
    pub fn host(words: &'a mut [u32], pads: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        let (task, irq) = oer_esp32s31_pac::usb_serial_jtag::usb_device_host(words)?;
        Ok(Self {
            task: Endpoint(task),
            irq,
            pads: UsbSerialJtagPads::host(pads)?,
        })
    }
}
