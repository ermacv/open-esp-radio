use bt_hci::PacketKind;

use super::{ControllerToHostPacketError, validate_controller_to_host_packet};

/// HCI_Command_Complete for HCI_Reset with status success.
const RESET_COMMAND_COMPLETE: [u8; 6] = [0x0e, 0x04, 0x01, 0x03, 0x0c, 0x00];

#[test]
fn a_complete_event_validates() {
    assert_eq!(
        validate_controller_to_host_packet(PacketKind::Event, &RESET_COMMAND_COMPLETE),
        Ok(())
    );
}

#[test]
fn invalid_direction_and_trailing_bytes_are_refused() {
    assert_eq!(
        validate_controller_to_host_packet(PacketKind::Cmd, &[0x03, 0x0c, 0x00]),
        Err(ControllerToHostPacketError::InvalidDirection)
    );
    assert_eq!(
        validate_controller_to_host_packet(PacketKind::Event, &[0x10, 0x00, 0xff]),
        Err(ControllerToHostPacketError::TrailingBytes)
    );
}

#[test]
fn every_controller_packet_header_rejects_declared_length_mismatch() {
    for (kind, bytes) in [
        (PacketKind::AclData, &[0x01, 0x00, 0x00, 0x00, 0xaa][..]),
        (PacketKind::SyncData, &[0x01, 0x00, 0x00, 0xbb][..]),
        (PacketKind::IsoData, &[0x01, 0x00, 0x00, 0x00, 0xcc][..]),
    ] {
        assert_eq!(
            validate_controller_to_host_packet(kind, bytes),
            Err(ControllerToHostPacketError::TrailingBytes)
        );
    }
    assert_eq!(
        validate_controller_to_host_packet(PacketKind::AclData, &[0x01, 0x00, 0x01]),
        Err(ControllerToHostPacketError::InvalidPacket(
            bt_hci::FromHciBytesError::InvalidSize
        ))
    );
}
