//! Complete Controller packet validation.

use super::ControllerToHostPacketError;
use bt_hci::{ControllerToHostPacket, FromHciBytesError, PacketKind};

fn decode_complete_packet(
    kind: PacketKind,
    bytes: &[u8],
) -> Result<ControllerToHostPacket<'_>, ControllerToHostPacketError> {
    validate_declared_length(kind, bytes)?;
    let (packet, remaining) = ControllerToHostPacket::from_hci_bytes_with_kind(kind, bytes)
        .map_err(ControllerToHostPacketError::InvalidPacket)?;
    if remaining.is_empty() {
        Ok(packet)
    } else {
        Err(ControllerToHostPacketError::TrailingBytes)
    }
}

fn validate_declared_length(
    kind: PacketKind,
    bytes: &[u8],
) -> Result<(), ControllerToHostPacketError> {
    let invalid_size =
        || ControllerToHostPacketError::InvalidPacket(FromHciBytesError::InvalidSize);
    let declared = match kind {
        PacketKind::Cmd => return Err(ControllerToHostPacketError::InvalidDirection),
        PacketKind::Event => {
            let Some(length) = bytes.get(1) else {
                return Err(ControllerToHostPacketError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            2_usize
                .checked_add(usize::from(*length))
                .ok_or_else(invalid_size)?
        }
        PacketKind::AclData => {
            let Some((length, _)) = bytes.get(2..).and_then(<[u8]>::split_first_chunk::<2>) else {
                return Err(ControllerToHostPacketError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            4_usize
                .checked_add(usize::from(u16::from_le_bytes(*length)))
                .ok_or_else(invalid_size)?
        }
        PacketKind::SyncData => {
            let Some(length) = bytes.get(2) else {
                return Err(ControllerToHostPacketError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            3_usize
                .checked_add(usize::from(*length))
                .ok_or_else(invalid_size)?
        }
        PacketKind::IsoData => {
            let Some((length, _)) = bytes.get(2..).and_then(<[u8]>::split_first_chunk::<2>) else {
                return Err(ControllerToHostPacketError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            let image = u16::from_le_bytes(*length);
            4_usize
                .checked_add(usize::from(image & 0x3fff))
                .ok_or_else(invalid_size)?
        }
    };

    if bytes.len() < declared {
        Err(ControllerToHostPacketError::InvalidPacket(
            FromHciBytesError::InvalidSize,
        ))
    } else if bytes.len() > declared {
        Err(ControllerToHostPacketError::TrailingBytes)
    } else {
        Ok(())
    }
}

pub(super) fn validate_complete_packet(
    kind: PacketKind,
    bytes: &[u8],
) -> Result<(), ControllerToHostPacketError> {
    decode_complete_packet(kind, bytes).map(|_| ())
}
