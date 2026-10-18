//! Complete packet validation and conversion at the async channel boundary.

use super::*;

pub(super) fn require_profile_buffer<const PACKET_CAPACITY: usize>(
    available: usize,
) -> Result<(), HciChannelError> {
    if available < PACKET_CAPACITY {
        Err(HciChannelError::DestinationTooSmall {
            required: PACKET_CAPACITY,
            available,
        })
    } else {
        Ok(())
    }
}

pub(super) fn controller_slot<const PACKET_CAPACITY: usize>(
    kind: PacketKind,
    bytes: &[u8],
) -> Result<PacketSlot<PACKET_CAPACITY>, HciChannelError> {
    if kind == PacketKind::Cmd {
        return Err(HciChannelError::InvalidDirection);
    }
    if bytes.len() > PACKET_CAPACITY {
        return Err(HciChannelError::PacketTooLong {
            length: bytes.len(),
            capacity: PACKET_CAPACITY,
        });
    }
    validate_controller_to_host_packet(kind, bytes).map_err(map_controller_packet_error)?;

    let mut slot = PacketSlot::EMPTY;
    slot.kind = kind;
    slot.length = bytes.len();
    slot.bytes[..bytes.len()].copy_from_slice(bytes);
    Ok(slot)
}

pub(super) fn decode_controller_slot<
    'buffer,
    P: PacketToHost<'buffer>,
    const PACKET_CAPACITY: usize,
>(
    mut slot: PacketSlot<PACKET_CAPACITY>,
    buffer: &'buffer mut [u8],
) -> Result<P, HciChannelError> {
    let result = {
        let mut reader = &slot.bytes[..slot.length];
        match P::read_hci(slot.kind, &mut reader, buffer) {
            Ok(packet) if reader.is_empty() => Ok(packet),
            Ok(_) => Err(HciChannelError::TrailingBytes),
            Err(error) => Err(HciChannelError::from(error)),
        }
    };
    slot.bytes[..slot.length].fill(0);
    result
}

fn map_controller_packet_error(error: ControllerToHostQueueError) -> HciChannelError {
    match error {
        ControllerToHostQueueError::InvalidDirection => HciChannelError::InvalidDirection,
        ControllerToHostQueueError::PacketTooLong { length, capacity } => {
            HciChannelError::PacketTooLong { length, capacity }
        }
        ControllerToHostQueueError::DestinationTooSmall {
            required,
            available,
        } => HciChannelError::DestinationTooSmall {
            required,
            available,
        },
        ControllerToHostQueueError::InvalidPacket(error) => HciChannelError::InvalidPacket(error),
        ControllerToHostQueueError::TrailingBytes => HciChannelError::TrailingBytes,
        ControllerToHostQueueError::Full
        | ControllerToHostQueueError::Empty
        | ControllerToHostQueueError::CorruptRetainedPacket => {
            HciChannelError::CorruptRetainedPacket
        }
    }
}

pub(super) fn encode_host_packet<T: PacketToController, const PACKET_CAPACITY: usize>(
    value: &T,
) -> Result<PacketSlot<PACKET_CAPACITY>, HciChannelError> {
    if T::KIND == PacketKind::Event {
        return Err(HciChannelError::InvalidDirection);
    }

    let declared = value.size();
    if declared > PACKET_CAPACITY {
        return Err(HciChannelError::PacketTooLong {
            length: declared,
            capacity: PACKET_CAPACITY,
        });
    }

    let mut slot = PacketSlot::EMPTY;
    let written = {
        let mut writer = PacketWriter::new(&mut slot.bytes);
        value.write_hci(&mut writer)?;
        writer.written()
    };
    if written != declared {
        slot.bytes[..written].fill(0);
        return Err(HciChannelError::SerializationLengthMismatch { declared, written });
    }
    validate_host_packet(T::KIND, &slot.bytes[..written])?;
    slot.kind = T::KIND;
    slot.length = written;
    Ok(slot)
}

pub(super) fn validate_host_packet(kind: PacketKind, bytes: &[u8]) -> Result<(), HciChannelError> {
    validate_host_declared_length(kind, bytes)?;
    match kind {
        PacketKind::Cmd => Ok(()),
        PacketKind::AclData => AclPacket::from_hci_bytes(bytes)
            .map_err(HciChannelError::InvalidPacket)
            .and_then(|(_, remaining)| require_no_remaining(remaining)),
        PacketKind::SyncData => SyncPacket::from_hci_bytes(bytes)
            .map_err(HciChannelError::InvalidPacket)
            .and_then(|(_, remaining)| require_no_remaining(remaining)),
        PacketKind::IsoData => IsoPacket::from_hci_bytes(bytes)
            .map_err(HciChannelError::InvalidPacket)
            .and_then(|(_, remaining)| require_no_remaining(remaining)),
        PacketKind::Event => Err(HciChannelError::InvalidDirection),
    }
}

fn validate_host_declared_length(kind: PacketKind, bytes: &[u8]) -> Result<(), HciChannelError> {
    let declared = match kind {
        PacketKind::Cmd => {
            let Some(length) = bytes.get(2) else {
                return Err(HciChannelError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            3 + usize::from(*length)
        }
        PacketKind::AclData => declared_u16_length(bytes, 4, 2, 0xffff)?,
        PacketKind::SyncData => {
            let Some(length) = bytes.get(2) else {
                return Err(HciChannelError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            3 + usize::from(*length)
        }
        PacketKind::IsoData => declared_u16_length(bytes, 4, 2, 0x3fff)?,
        PacketKind::Event => return Err(HciChannelError::InvalidDirection),
    };

    if bytes.len() < declared {
        Err(HciChannelError::InvalidPacket(
            FromHciBytesError::InvalidSize,
        ))
    } else if bytes.len() > declared {
        Err(HciChannelError::TrailingBytes)
    } else {
        Ok(())
    }
}

fn declared_u16_length(
    bytes: &[u8],
    header_length: usize,
    length_offset: usize,
    mask: u16,
) -> Result<usize, HciChannelError> {
    let Some(length) = bytes.get(length_offset..length_offset + 2) else {
        return Err(HciChannelError::InvalidPacket(
            FromHciBytesError::InvalidSize,
        ));
    };
    Ok(header_length + usize::from(u16::from_le_bytes([length[0], length[1]]) & mask))
}

fn require_no_remaining(remaining: &[u8]) -> Result<(), HciChannelError> {
    if remaining.is_empty() {
        Ok(())
    } else {
        Err(HciChannelError::TrailingBytes)
    }
}

pub(super) fn decode_host_slot<'buffer, const PACKET_CAPACITY: usize>(
    mut slot: PacketSlot<PACKET_CAPACITY>,
    buffer: &'buffer mut [u8],
) -> Result<HostToControllerFrame<'buffer>, HciChannelError> {
    buffer[..slot.length].copy_from_slice(&slot.bytes[..slot.length]);
    slot.bytes[..slot.length].fill(0);
    let bytes = &buffer[..slot.length];
    validate_host_packet(slot.kind, bytes).map_err(|_| HciChannelError::CorruptRetainedPacket)?;

    match slot.kind {
        PacketKind::Cmd => HciCommandPacket::from_hci_bytes(bytes)
            .map(HostToControllerFrame::Command)
            .map_err(|_| HciChannelError::CorruptRetainedPacket),
        PacketKind::AclData => AclPacket::from_hci_bytes(bytes)
            .map(|(packet, _)| HostToControllerFrame::Acl(packet))
            .map_err(|_| HciChannelError::CorruptRetainedPacket),
        PacketKind::SyncData => SyncPacket::from_hci_bytes(bytes)
            .map(|(packet, _)| HostToControllerFrame::Sync(packet))
            .map_err(|_| HciChannelError::CorruptRetainedPacket),
        PacketKind::IsoData => IsoPacket::from_hci_bytes(bytes)
            .map(|(packet, _)| HostToControllerFrame::Iso(packet))
            .map_err(|_| HciChannelError::CorruptRetainedPacket),
        PacketKind::Event => Err(HciChannelError::CorruptRetainedPacket),
    }
}

struct PacketWriter<'buffer> {
    buffer: &'buffer mut [u8],
    written: usize,
}

impl<'buffer> PacketWriter<'buffer> {
    fn new(buffer: &'buffer mut [u8]) -> Self {
        Self { buffer, written: 0 }
    }

    fn written(&self) -> usize {
        self.written
    }
}

impl ErrorType for PacketWriter<'_> {
    type Error = HciChannelError;
}

impl Write for PacketWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        if bytes.len() > self.buffer.len().saturating_sub(self.written) {
            return Err(HciChannelError::SerializationOverflow {
                capacity: self.buffer.len(),
            });
        }
        let end = self.written + bytes.len();
        self.buffer[self.written..end].copy_from_slice(bytes);
        self.written = end;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
