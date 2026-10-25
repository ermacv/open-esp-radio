//! The USB Serial/JTAG console's blocks: the data FIFO with its
//! configuration and interrupts, and the pads.
//!
//! One handoff proof makes the owner of both blocks, which divides into the
//! task's endpoint, the interrupt handler's acknowledgement and the pads.
//! Unlike the other platform domains the block has an owner before esp-hal
//! exists: the early writer, which builds one for each record it writes,
//! until the console's driver takes the block from the esp-hal adapter; and
//! the fatal path, which takes only a block no one holds.

//!
//! The generated IRQ part acknowledges an affine sample exactly once:
//!
//! ```compile_fail,E0382
//! use oer_esp32s31_pac::usb_serial_jtag::UsbSerialJtagIrq;
//! fn twice(irq: &mut UsbSerialJtagIrq) {
//!     let sample = irq.sample_usb_console_interrupt();
//!     irq.acknowledge_usb_console_interrupt(sample);
//!     irq.acknowledge_usb_console_interrupt(sample);
//! }
//! ```
//!
//! No raw clear-word writer bypasses that sample:
//!
//! ```compile_fail,E0599
//! use oer_esp32s31_pac::usb_serial_jtag::UsbSerialJtagIrq;
//! fn raw_ack(irq: &mut UsbSerialJtagIrq) {
//!     irq.write_int_clr(|_| {});
//! }
//! ```

use oer_mmio::{Fixed, Placement};

use crate::generated::{
    peripheral_ownership::{UsbSerialJtagPeripherals, UsbSerialJtagPeripheralsClaims},
    usb_serial_jtag::{self, UsbSerialJtag},
    usb_serial_jtag_pads::UsbSerialJtagPads,
};

/// Proof that the caller holds the USB Serial/JTAG words alone.
#[must_use = "the USB Serial/JTAG owner needs the platform's transfer"]
pub struct UsbSerialJtagHandoff(());

impl UsbSerialJtagHandoff {
    /// Assert that the caller holds the USB Serial/JTAG block alone.
    ///
    /// # Safety
    /// While the owner built from this proof, or any part of it, lives, no
    /// other code accesses the block's words: no owner built from another
    /// proof lives at the same time, and no esp-hal owner of `USB_DEVICE`
    /// writes them.
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
    pub endpoint: UsbSerialJtag<P>,
    pub pads: UsbSerialJtagPads<P>,
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
