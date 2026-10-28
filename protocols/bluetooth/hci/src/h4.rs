//! H4, the UART transport framing of HCI packets: one packet indicator octet
//! before each packet.
//!
//! SOURCE: [Bluetooth Core 6.0, Vol 4, Part A, 2](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-60/out/en/host-controller-interface/uart-transport-layer.html),
//! Table 2.1 (packet indicators: command 0x01, ACL data 0x02, synchronous data
//! 0x03, event 0x04, ISO data 0x05), and Vol 4, Part E, 5.4.1 to 5.4.5 for the
//! header that carries each packet's payload length.
//!
//! The codec is one place for the indicator values, the split of a received
//! frame, the extent of a frame from its header and the build of a frame.
//! Waiting, gaps between octets and the serial port belong to the transport
//! that owns the wire.

use bt_hci::{FromHciBytesError, PacketKind, cmd::Opcode};
use oer_wire::{Reader, Writer};

use crate::HciCommandPacket;

/// Core 6.0, Vol 4, Part A, Table 2.1: HCI Command packet.
const COMMAND: u8 = 0x01;
/// Core 6.0, Vol 4, Part A, Table 2.1: HCI ACL Data packet.
const ACL_DATA: u8 = 0x02;
/// Core 6.0, Vol 4, Part A, Table 2.1: HCI Synchronous Data packet.
const SYNC_DATA: u8 = 0x03;
/// Core 6.0, Vol 4, Part A, Table 2.1: HCI Event packet.
const EVENT: u8 = 0x04;
/// Core 6.0, Vol 4, Part A, Table 2.1: HCI ISO Data packet.
const ISO_DATA: u8 = 0x05;

/// Core 6.0, Vol 4, Part E, 5.4.1: opcode (2 octets) and parameter length
/// (1 octet); the length is the third header octet.
const COMMAND_HEADER: usize = 3;
/// Core 6.0, Vol 4, Part E, 5.4.2: handle and flags (2 octets) and data total
/// length (2 octets, little endian).
const ACL_HEADER: usize = 4;
/// Core 6.0, Vol 4, Part E, 5.4.3: handle and flags (2 octets) and data total
/// length (1 octet).
const SYNC_HEADER: usize = 3;
/// Core 6.0, Vol 4, Part E, 5.4.4: event code (1 octet) and parameter total
/// length (1 octet).
const EVENT_HEADER: usize = 2;
/// Core 6.0, Vol 4, Part E, 5.4.5: handle and flags (2 octets) and the 14-bit
/// ISO data load length with two reserved bits (2 octets).
const ISO_HEADER: usize = 4;
/// Core 6.0, Vol 4, Part E, 5.4.5: the ISO data load length is the 14 bits
/// after the first two header octets.
const ISO_LENGTH_BITS: usize = 14;

/// The longest command frame: the indicator, the three header octets and the
/// 255 parameter octets a one-octet length allows.
pub const COMMAND_FRAME_CAPACITY: usize = 1 + COMMAND_HEADER + u8::MAX as usize;

/// Why bytes are not a valid H4 frame, or why a frame cannot be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// The first octet is not a packet indicator of Table 2.1.
    UnknownIndicator(u8),
    /// The stream carries commands only, and its next frame has another kind.
    NotCommand(PacketKind),
    /// The cursor refused the input or the destination.
    Wire(oer_wire::Error),
    /// A command's parameters do not fit its one-octet length.
    ParametersTooLong,
    /// A complete command still awaits consumption, so no octet may follow.
    Unconsumed,
    /// The command is not complete.
    Incomplete,
    /// The octets exceed the longest frame their kind allows.
    FrameTooLong,
    /// A complete command frame does not decode as an HCI command.
    Command(FromHciBytesError),
}

impl From<oer_wire::Error> for Error {
    fn from(error: oer_wire::Error) -> Self {
        Self::Wire(error)
    }
}

/// The indicator octet of `kind`.
pub const fn indicator(kind: PacketKind) -> u8 {
    match kind {
        PacketKind::Cmd => COMMAND,
        PacketKind::AclData => ACL_DATA,
        PacketKind::SyncData => SYNC_DATA,
        PacketKind::Event => EVENT,
        PacketKind::IsoData => ISO_DATA,
    }
}

/// The packet kind an indicator octet names.
pub const fn kind(indicator: u8) -> Result<PacketKind, Error> {
    match indicator {
        COMMAND => Ok(PacketKind::Cmd),
        ACL_DATA => Ok(PacketKind::AclData),
        SYNC_DATA => Ok(PacketKind::SyncData),
        EVENT => Ok(PacketKind::Event),
        ISO_DATA => Ok(PacketKind::IsoData),
        other => Err(Error::UnknownIndicator(other)),
    }
}

/// Split a frame into its packet kind and the packet that follows the
/// indicator. The packet's own header declares its extent: use
/// [`frame_length`] to find the end of a frame in a stream, or the packet's
/// codec to reject trailing bytes.
pub fn split(frame: &[u8]) -> Result<(PacketKind, &[u8]), Error> {
    let mut reader = Reader::new(frame);
    let kind = kind(reader.u8()?)?;
    Ok((kind, reader.remaining()))
}

/// The length of the frame that starts `prefix`, indicator included, once the
/// header holds it; `None` while the prefix ends inside the header.
pub fn frame_length(prefix: &[u8]) -> Result<Option<usize>, Error> {
    let (kind, header) = match split(prefix) {
        Ok(split) => split,
        Err(Error::Wire(oer_wire::Error::Truncated { .. })) => return Ok(None),
        Err(error) => return Err(error),
    };
    let (header_length, payload) = match kind {
        PacketKind::Cmd => (COMMAND_HEADER, payload_length(header, 2, Width::Octet)),
        PacketKind::AclData => (ACL_HEADER, payload_length(header, 2, Width::Short)),
        PacketKind::SyncData => (SYNC_HEADER, payload_length(header, 2, Width::Octet)),
        PacketKind::Event => (EVENT_HEADER, payload_length(header, 1, Width::Octet)),
        PacketKind::IsoData => (ISO_HEADER, payload_length(header, 2, Width::Iso)),
    };
    let payload = match payload {
        Ok(payload) => payload,
        Err(oer_wire::Error::Truncated { .. }) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let length = 1_usize
        .checked_add(header_length)
        .and_then(|length| length.checked_add(payload))
        .ok_or(oer_wire::Error::LengthOverflow)?;
    Ok(Some(length))
}

/// The encoding of a header's payload length.
#[derive(Clone, Copy)]
enum Width {
    Octet,
    Short,
    Iso,
}

/// Read the payload length that starts `offset` octets into `header`.
fn payload_length(header: &[u8], offset: usize, width: Width) -> Result<usize, oer_wire::Error> {
    let mut reader = Reader::new(header);
    reader.take(offset)?;
    let length = match width {
        Width::Octet => usize::from(reader.u8()?),
        Width::Short => usize::from(reader.u16_le()?),
        Width::Iso => {
            let bits = reader.bits::<ISO_LENGTH_BITS>(0)?;
            usize::try_from(bits).map_err(|_| oer_wire::Error::LengthOverflow)?
        }
    };
    Ok(length)
}

/// Write the indicator of `kind` and then `packet` into `out`; the number of
/// octets written.
pub fn write(kind: PacketKind, packet: &[u8], out: &mut [u8]) -> Result<usize, Error> {
    let mut writer = Writer::new(out);
    writer.u8(indicator(kind))?;
    writer.write(packet)?;
    Ok(writer.written())
}

/// Write a command frame: the indicator, the opcode and parameter length of
/// Core 6.0, Vol 4, Part E, 5.4.1, and the parameters; the number of octets
/// written.
pub fn write_command(opcode: Opcode, parameters: &[u8], out: &mut [u8]) -> Result<usize, Error> {
    let length = u8::try_from(parameters.len()).map_err(|_| Error::ParametersTooLong)?;
    let mut writer = Writer::new(out);
    writer.u8(indicator(PacketKind::Cmd))?;
    writer.u16_le(opcode.to_raw())?;
    writer.u8(length)?;
    writer.write(parameters)?;
    Ok(writer.written())
}

/// Assembles one H4 command frame from the octets of a serial stream, one
/// octet at a time. The frame stays until it is consumed, so a following
/// octet never replaces an undelivered command.
pub struct CommandReceiver {
    bytes: [u8; COMMAND_FRAME_CAPACITY],
    used: usize,
    expected: Option<usize>,
}

impl CommandReceiver {
    pub const fn new() -> Self {
        Self {
            bytes: [0; COMMAND_FRAME_CAPACITY],
            used: 0,
            expected: None,
        }
    }

    /// A frame has begun: its indicator arrived.
    pub const fn started(&self) -> bool {
        self.used != 0
    }

    /// The frame is complete and awaits [`consume`](Self::consume).
    pub fn complete(&self) -> bool {
        self.expected == Some(self.used)
    }

    /// Add the next octet; `true` when it completes the frame. An octet that
    /// begins a frame must be the command indicator, and a refused octet
    /// leaves the receiver unchanged.
    pub fn push(&mut self, byte: u8) -> Result<bool, Error> {
        if self.complete() {
            return Err(Error::Unconsumed);
        }
        if !self.started() {
            let kind = kind(byte)?;
            if kind != PacketKind::Cmd {
                return Err(Error::NotCommand(kind));
            }
        }
        let used = self.used.checked_add(1).ok_or(Error::FrameTooLong)?;
        *self.bytes.get_mut(self.used).ok_or(Error::FrameTooLong)? = byte;
        self.used = used;
        if self.expected.is_none() {
            let prefix = self.bytes.get(..used).ok_or(Error::FrameTooLong)?;
            self.expected = frame_length(prefix)?;
        }
        Ok(self.complete())
    }

    /// The complete command.
    pub fn packet(&self) -> Result<HciCommandPacket<'_>, Error> {
        if !self.complete() {
            return Err(Error::Incomplete);
        }
        let (_, packet) = split(self.bytes.get(..self.used).ok_or(Error::FrameTooLong)?)?;
        HciCommandPacket::from_hci_bytes(packet).map_err(Error::Command)
    }

    /// Release a complete command, after its consumer copied what it needs.
    pub fn consume(&mut self) -> Result<(), Error> {
        if !self.complete() {
            return Err(Error::Incomplete);
        }
        self.reset();
        Ok(())
    }

    /// Drop any frame, complete or not.
    pub fn reset(&mut self) {
        self.used = 0;
        self.expected = None;
    }
}

impl Default for CommandReceiver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
