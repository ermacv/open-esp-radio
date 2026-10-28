use super::*;
use bt_hci::{
    FromHciBytes,
    cmd::{Cmd, controller_baseband::Reset, le::LeSetRandomAddr},
};

const KINDS: [PacketKind; 5] = [
    PacketKind::Cmd,
    PacketKind::AclData,
    PacketKind::SyncData,
    PacketKind::Event,
    PacketKind::IsoData,
];

#[test]
fn indicators_round_trip_and_agree_with_bt_hci() {
    for kind_value in KINDS {
        let byte = indicator(kind_value);
        assert_eq!(kind(byte), Ok(kind_value));
        let bytes = [byte];
        let (decoded, rest) = PacketKind::from_hci_bytes(&bytes).unwrap();
        assert_eq!((decoded, rest), (kind_value, &[][..]));
    }
}

#[test]
fn unknown_indicators_are_refused() {
    for byte in [0, 6, 0x7f, 0xff] {
        assert_eq!(kind(byte), Err(Error::UnknownIndicator(byte)));
        assert_eq!(split(&[byte, 0]), Err(Error::UnknownIndicator(byte)));
        assert_eq!(frame_length(&[byte, 0]), Err(Error::UnknownIndicator(byte)));
    }
}

#[test]
fn split_returns_the_kind_and_the_rest() {
    assert_eq!(
        split(&[4, 0x0e, 1, 2]),
        Ok((PacketKind::Event, &[0x0e, 1, 2][..]))
    );
    assert_eq!(split(&[1]), Ok((PacketKind::Cmd, &[][..])));
    assert!(matches!(
        split(&[]),
        Err(Error::Wire(oer_wire::Error::Truncated { .. }))
    ));
}

#[test]
fn write_then_split_round_trips_every_kind() {
    for kind_value in KINDS {
        let mut out = [0; 8];
        let length = write(kind_value, &[9, 8, 7], &mut out).unwrap();
        assert_eq!(
            out.get(..length),
            Some(&[indicator(kind_value), 9, 8, 7][..])
        );
        assert_eq!(split(&out[..length]), Ok((kind_value, &[9, 8, 7][..])));
    }
}

#[test]
fn write_refuses_a_destination_that_is_too_short() {
    let mut out = [0; 3];
    assert!(matches!(
        write(PacketKind::Event, &[1, 2, 3], &mut out),
        Err(Error::Wire(oer_wire::Error::Truncated { .. }))
    ));
}

#[test]
fn a_written_command_decodes_as_the_same_command() {
    let mut out = [0; COMMAND_FRAME_CAPACITY];
    let parameters = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
    let length = write_command(LeSetRandomAddr::OPCODE, &parameters, &mut out).unwrap();
    assert_eq!(length, 1 + 3 + parameters.len());
    assert_eq!(frame_length(&out[..length]), Ok(Some(length)));
    let (kind_value, packet) = split(&out[..length]).unwrap();
    assert_eq!(kind_value, PacketKind::Cmd);
    let command = HciCommandPacket::from_hci_bytes(packet).unwrap();
    assert_eq!(command.opcode(), LeSetRandomAddr::OPCODE);
    assert_eq!(command.parameters(), parameters);
}

#[test]
fn a_command_with_too_many_parameters_is_refused() {
    let mut out = [0; 512];
    assert_eq!(
        write_command(Reset::OPCODE, &[0; 256], &mut out),
        Err(Error::ParametersTooLong)
    );
    assert!(write_command(Reset::OPCODE, &[0; 255], &mut out).is_ok());
}

#[test]
fn the_extent_of_each_kind_comes_from_its_own_header() {
    // Command: opcode, one-octet length.
    assert_eq!(frame_length(&[1, 3, 12, 2]), Ok(Some(1 + 3 + 2)));
    // ACL: handle, two-octet little-endian length.
    assert_eq!(frame_length(&[2, 1, 0, 0x00, 0x01]), Ok(Some(1 + 4 + 256)));
    // Synchronous: handle, one-octet length.
    assert_eq!(frame_length(&[3, 1, 0, 5]), Ok(Some(1 + 3 + 5)));
    // Event: code, one-octet length.
    assert_eq!(frame_length(&[4, 0x0e, 4]), Ok(Some(1 + 2 + 4)));
    // ISO: handle, a 14-bit length whose reserved top bits are ignored.
    assert_eq!(
        frame_length(&[5, 1, 0, 0x34, 0xc2]),
        Ok(Some(1 + 4 + 0x234))
    );
}

#[test]
fn the_extent_waits_for_the_whole_header() {
    for prefix in [
        &[][..],
        &[1],
        &[1, 3],
        &[1, 3, 12],
        &[2, 1, 0, 0],
        &[3, 1, 0],
        &[4],
        &[4, 0x0e],
        &[5, 1, 0, 1],
    ] {
        assert_eq!(frame_length(prefix), Ok(None), "{prefix:?}");
    }
}

fn feed(receiver: &mut CommandReceiver, bytes: &[u8]) {
    for byte in bytes {
        receiver.push(*byte).unwrap();
    }
}

#[test]
fn fragmented_reset_is_published_only_after_its_complete_header() {
    let mut receiver = CommandReceiver::new();
    for byte in [1, 3, 12] {
        assert_eq!(receiver.push(byte), Ok(false));
        assert_eq!(receiver.packet().err(), Some(Error::Incomplete));
    }
    assert_eq!(receiver.push(0), Ok(true));
    let packet = receiver.packet().unwrap();
    assert_eq!(packet.opcode(), Reset::OPCODE);
    assert!(packet.parameters().is_empty());
}

#[test]
fn maximum_command_keeps_all_parameter_bytes() {
    let mut receiver = CommandReceiver::new();
    for byte in [1, 0, 252, 255] {
        assert_eq!(receiver.push(byte), Ok(false));
    }
    for value in 0..255_u8 {
        assert_eq!(receiver.push(value), Ok(value == 254));
    }
    let packet = receiver.packet().unwrap();
    assert_eq!(packet.parameters().len(), 255);
    assert!(packet.parameters().iter().copied().eq(0..255_u8));
}

#[test]
fn a_complete_command_cannot_be_overwritten_by_the_next_frame() {
    let mut receiver = CommandReceiver::new();
    feed(&mut receiver, &[1, 3, 12, 0]);
    assert_eq!(receiver.push(1), Err(Error::Unconsumed));
    assert_eq!(receiver.packet().unwrap().opcode(), Reset::OPCODE);
    receiver.consume().unwrap();
    assert_eq!(receiver.push(1), Ok(false));
}

#[test]
fn a_wrong_indicator_is_refused_without_starting_a_frame() {
    let mut receiver = CommandReceiver::new();
    assert_eq!(
        receiver.push(2),
        Err(Error::NotCommand(PacketKind::AclData))
    );
    assert_eq!(receiver.push(0x40), Err(Error::UnknownIndicator(0x40)));
    assert!(!receiver.started());
    feed(&mut receiver, &[1, 3, 12, 0]);
    assert_eq!(receiver.packet().unwrap().opcode(), Reset::OPCODE);
}

#[test]
fn premature_consumption_does_not_destroy_a_partial_header() {
    let mut receiver = CommandReceiver::new();
    feed(&mut receiver, &[1, 3]);
    assert_eq!(receiver.consume(), Err(Error::Incomplete));
    assert_eq!(receiver.push(12), Ok(false));
    assert_eq!(receiver.push(0), Ok(true));
    assert_eq!(receiver.packet().unwrap().opcode(), Reset::OPCODE);
}

#[test]
fn reset_drops_a_partial_frame() {
    let mut receiver = CommandReceiver::new();
    feed(&mut receiver, &[1, 3, 12]);
    assert!(receiver.started());
    receiver.reset();
    assert!(!receiver.started());
    feed(&mut receiver, &[1, 3, 12, 0]);
    assert!(receiver.complete());
}
