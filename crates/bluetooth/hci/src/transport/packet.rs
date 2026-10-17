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
    let declared = match kind {
        PacketKind::Cmd => return Err(ControllerToHostPacketError::InvalidDirection),
        PacketKind::Event => {
            let Some(length) = bytes.get(1) else {
                return Err(ControllerToHostPacketError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            2 + usize::from(*length)
        }
        PacketKind::AclData => {
            let Some(length) = bytes.get(2..4) else {
                return Err(ControllerToHostPacketError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            4 + usize::from(u16::from_le_bytes([length[0], length[1]]))
        }
        PacketKind::SyncData => {
            let Some(length) = bytes.get(2) else {
                return Err(ControllerToHostPacketError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            3 + usize::from(*length)
        }
        PacketKind::IsoData => {
            let Some(length) = bytes.get(2..4) else {
                return Err(ControllerToHostPacketError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            let image = u16::from_le_bytes([length[0], length[1]]);
            4 + usize::from(image & 0x3fff)
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
