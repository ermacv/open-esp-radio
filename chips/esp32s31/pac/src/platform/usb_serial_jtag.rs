//! The USB Serial/JTAG console's blocks: the data FIFO with its
//! configuration and interrupts, and the pads.
//!
//! One handoff proof makes the owner of both blocks, which divides into the
//! task's endpoint, the interrupt handler's acknowledgement and the pads.
//! Unlike the other platform domains the block has an owner before esp-hal
//! exists: the early writer, which builds one for each record it writes,
//! until the console's driver takes the block from the esp-hal adapter; and
//! the fatal path, which takes it from a driver that never runs again.

use oer_mmio::{Fixed, Placement};

use crate::generated::{
    peripheral_ownership::{UsbSerialJtagPeripherals, UsbSerialJtagPeripheralsClaims},
    usb_serial_jtag::{self, UsbSerialJtag, UsbSerialJtagIrq, UsbSerialJtagTask},
    usb_serial_jtag_pads::UsbSerialJtagPads,
};

#[cfg(test)]
mod tests;

/// Proof that the caller holds the USB Serial/JTAG words alone.
#[must_use = "the USB Serial/JTAG owner needs the platform's transfer"]
pub struct UsbSerialJtagHandoff(());

impl UsbSerialJtagHandoff {
    /// Assert that the caller holds the USB Serial/JTAG block alone.
    ///
    /// # Safety
    /// While the owner built from this proof, or any part of it, lives, no
    /// other code accesses the block's words, and no esp-hal owner of
    /// `USB_DEVICE` writes them. An owner built from another proof lives at
    /// the same time only if it never accesses the block again from the
    /// moment this proof is made: the code that holds it is suspended for
    /// good, as the console's driver is under the fatal takeover.
    #[allow(
        unsafe_code,
        reason = "the platform proves exclusive USB Serial/JTAG ownership"
    )]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// Where the device's USB Serial/JTAG words lie.
pub type UsbSerialJtagPlacement = Fixed<{ usb_serial_jtag::BASE }>;

/// The USB Serial/JTAG console's blocks.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct UsbSerialJtagRegisters<P: Placement = UsbSerialJtagPlacement> {
    endpoint: UsbSerialJtag<P>,
    pads: UsbSerialJtagPads<P>,
}

impl UsbSerialJtagRegisters {
    /// Claim both blocks from the platform handoff.
    #[allow(
        unsafe_code,
        reason = "the consumed handoff proves unique register ownership"
    )]
    pub fn from_handoff(_handoff: UsbSerialJtagHandoff) -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "the chip PAC's root claims a domain's typed blocks, from its handoff"
        )]
        // SAFETY: `UsbSerialJtagHandoff`'s constructor guarantees that no
        // other code accesses the block's words while this owner lives. The
        // generated `const` assertion beside the typed blocks checks that no
        // other typed block lists them.
        let claims = unsafe { oer_mmio::assume_owned::<UsbSerialJtagPeripheralsClaims>() };
        let blocks = UsbSerialJtagPeripherals::from_claims(claims);
        Self {
            endpoint: blocks.usb_serial_jtag,
            pads: blocks.usb_serial_jtag_pads,
        }
    }
}

impl<P: Placement> UsbSerialJtagRegisters<P> {
    /// The task's endpoint, the interrupt handler's acknowledgement and the
    /// pads, three disjoint owners.
    pub fn into_parts(
        self,
    ) -> (
        UsbEndpointRegisters<P>,
        UsbInterruptRegisters<P>,
        UsbPadRegisters<P>,
    ) {
        let (task, irq) = self.endpoint.split();
        (
            UsbEndpointRegisters(task),
            UsbInterruptRegisters(irq),
            UsbPadRegisters(self.pads),
        )
    }
}

/// The interrupt events of the serial endpoints the console uses.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UsbEvents {
    /// The IN FIFO became empty: the host read the last packet.
    pub in_empty: bool,
    /// The OUT endpoint received a packet.
    pub out_received: bool,
}

/// The task's side: the data FIFO, its configuration and the interrupt
/// enables.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct UsbEndpointRegisters<P: Placement = UsbSerialJtagPlacement>(UsbSerialJtagTask<P>);

impl<P: Placement> UsbEndpointRegisters<P> {
    /// Whether the IN FIFO has room for a byte. After [`send`](Self::send)
    /// it has none until the host read the packet.
    pub fn has_room(&self) -> bool {
        self.0.read_ep1_conf().serial_in_ep_data_free()
    }

    /// Append `byte` to the IN FIFO; call only while it has room.
    pub fn push(&mut self, byte: u8) {
        self.0.push_ep1(byte);
    }

    /// Hand the bytes in the IN FIFO to the host as one packet; with none,
    /// a zero-length packet.
    pub fn send(&mut self) {
        self.0.trigger_ep1_conf_wr_done();
    }

    /// Whether the OUT FIFO holds a byte.
    pub fn has_data(&self) -> bool {
        self.0.read_ep1_conf().serial_out_ep_data_avail()
    }

    /// Take one byte from the OUT FIFO; call only while it holds one.
    pub fn pop(&mut self) -> u8 {
        self.0.pop_ep1()
    }

    /// Enable both console interrupts, keeping every other enable.
    pub fn enable_events(&mut self) {
        self.0.modify_int_ena(|enables| {
            enables
                .set_serial_in_empty_int_ena(true)
                .set_serial_out_recv_pkt_int_ena(true);
        });
    }
}

/// The interrupt handler's side: the interrupt status and its
/// acknowledgement.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct UsbInterruptRegisters<P: Placement = UsbSerialJtagPlacement>(UsbSerialJtagIrq<P>);

impl<P: Placement> UsbInterruptRegisters<P> {
    /// Read the pending console events and acknowledge exactly those.
    pub fn take_events(&mut self) -> UsbEvents {
        let status = self.0.read_int_st();
        let events = UsbEvents {
            in_empty: status.serial_in_empty_int_st(),
            out_received: status.serial_out_recv_pkt_int_st(),
        };
        if events != UsbEvents::default() {
            self.0.write_int_clr(|clear| {
                clear
                    .set_serial_in_empty_int_clr(events.in_empty)
                    .set_serial_out_recv_pkt_int_clr(events.out_received);
            });
        }
        events
    }
}

/// The pads of the USB Serial/JTAG.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct UsbPadRegisters<P: Placement = UsbSerialJtagPlacement>(UsbSerialJtagPads<P>);

impl<P: Placement> UsbPadRegisters<P> {
    /// Take the device off its pads and drop the D+ and D- pull-ups: the
    /// host sees it leave USB. Only a reset of the chip, which restores the
    /// pads for the ROM, brings it back. Every other bit of the pad word
    /// keeps its value.
    pub fn disconnect(mut self) {
        self.0.modify_conf0(|pads| {
            pads.set_pad_pull_override(true)
                .set_dp_pullup(false)
                .set_dm_pullup(false)
                .set_usb_pad_enable(false);
        });
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> UsbSerialJtagRegisters<oer_mmio::Host<'a>> {
    /// Each block over its own host memory, which holds the words from the
    /// device's base to the block's last one.
    pub fn host(
        endpoint: &'a mut [u32],
        pads: &'a mut [u32],
    ) -> Result<Self, oer_mmio::HostTooSmall> {
        Ok(Self {
            endpoint: UsbSerialJtag::host(endpoint)?,
            pads: UsbSerialJtagPads::host(pads)?,
        })
    }
}
