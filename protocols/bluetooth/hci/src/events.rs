//! Fixed Controller event codecs, without transport framing or failure policy.
//!
//! SOURCE: [Bluetooth Core 6.0, Vol 4, Part E](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-60/out/en/host-controller-interface/host-controller-interface-functional-specification.html),
//! sections 5.4.4 (event envelope), 7.7.14 (Command Complete), and 7.7.16
//! (Hardware Error). Section 5.4.4 reserves 0xff for vendor debug events;
//! the shutdown subevent and its observation bytes are the B0 product dialect.

use bt_hci::{PacketKind, cmd::Opcode, param::Status};
use oer_wire::{U8, U16};

use crate::HciControllerResponse;

oer_wire::bitfield! {
    struct HardwareErrorFields[3] (little) {
        /// Core 6.0, Vol 4, Part E, 5.4.4 and 7.7.16: event code 0x10.
        code, with_code: (u8) = 0..8;
        /// Core 6.0, Vol 4, Part E, 5.4.4: one parameter octet.
        length, with_length: (u8) = 8..16;
        /// Core 6.0, Vol 4, Part E, 7.7.16: implementation-specific code.
        hardware_code, with_hardware_code: (u8) = 16..24;
    }
}

/// Complete Hardware Error event; the caller owns the code's meaning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HardwareErrorEvent([u8; 3]);

impl HardwareErrorEvent {
    pub const fn new(hardware_code: u8) -> Self {
        Self(
            HardwareErrorFields::from_bytes([0; 3])
                .with_code(U8::constant::<0x10>())
                .with_length(U8::constant::<1>())
                // Full-octet fields preserve every input bit.
                .with_hardware_code(U8::from_low_bits(hardware_code))
                .to_bytes(),
        )
    }
}

impl HciControllerResponse for HardwareErrorEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

oer_wire::bitfield! {
    struct CommandCompleteStatusFields[6] (little) {
        /// Core 6.0, Vol 4, Part E, 5.4.4 and 7.7.14: event code 0x0e.
        code, with_code: (u8) = 0..8;
        /// Core 6.0, Vol 4, Part E, 5.4.4: four parameter octets.
        length, with_length: (u8) = 8..16;
        /// Core 6.0, Vol 4, Part E, 7.7.14: one available command credit.
        credits, with_credits: (u8) = 16..24;
        /// Core 6.0, Vol 4, Part E, 7.7.14: completed command, little endian.
        opcode, with_opcode: (u16) = 24..40;
        /// Core 6.0, Vol 4, Part E, 7.7.14: command's Status return parameter.
        status, with_status: (u8) = 40..48;
    }
}

/// One-credit Command Complete for a command returning only Status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandCompleteStatusEvent([u8; 6]);

impl CommandCompleteStatusEvent {
    pub fn new(opcode: Opcode, status: Status) -> Self {
        Self(
            CommandCompleteStatusFields::from_bytes([0; 6])
                .with_code(U8::constant::<0x0e>())
                .with_length(U8::constant::<4>())
                .with_credits(U8::constant::<1>())
                .with_opcode(U16::from_low_bits(opcode.to_raw()))
                .with_status(U8::from_low_bits(status.into_inner()))
                .to_bytes(),
        )
    }
}

impl HciControllerResponse for CommandCompleteStatusEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

oer_wire::bitfield! {
    struct ShutdownObservationFields[5] (little) {
        /// Core 6.0, Vol 4, Part E, 5.4.4: vendor event code 0xff.
        code, with_code: (u8) = 0..8;
        /// Core 6.0, Vol 4, Part E, 5.4.4: three parameter octets.
        length, with_length: (u8) = 8..16;
        /// B0 product dialect: shutdown observation subevent 1.
        subevent, with_subevent: (u8) = 16..24;
        /// B0 product dialect: software-pending observation, unmodified.
        software, with_software: (u8) = 24..32;
        /// B0 product dialect: rollover-pending observation, unmodified.
        rollover, with_rollover: (u8) = 32..40;
    }
}

/// Vendor shutdown observation, carrying two opaque pending-status octets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VendorShutdownObservationEvent([u8; 5]);

impl VendorShutdownObservationEvent {
    pub const fn new([software, rollover]: [u8; 2]) -> Self {
        Self(
            ShutdownObservationFields::from_bytes([0; 5])
                .with_code(U8::constant::<0xff>())
                .with_length(U8::constant::<3>())
                .with_subevent(U8::constant::<1>())
                .with_software(U8::from_low_bits(software))
                .with_rollover(U8::from_low_bits(rollover))
                .to_bytes(),
        )
    }

    /// Decode exactly this product event body, without an H4 indicator.
    pub fn from_hci_bytes(bytes: &[u8]) -> Option<Self> {
        let [_, _, _, _, _] = bytes else {
            return None;
        };
        let bytes = oer_wire::Reader::new(bytes).read::<5>().ok()?;
        let fields = ShutdownObservationFields::from_bytes(bytes);
        if fields.code().get() != 0xff || fields.length().get() != 3 || fields.subevent().get() != 1
        {
            return None;
        }
        Some(Self(bytes))
    }

    /// The sampled software-pending and rollover-pending octets, in that order.
    pub const fn pending(&self) -> [u8; 2] {
        let fields = ShutdownObservationFields::from_bytes(self.0);
        [fields.software().get(), fields.rollover().get()]
    }
}

impl HciControllerResponse for VendorShutdownObservationEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }
    fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[cfg(test)]
mod tests;
