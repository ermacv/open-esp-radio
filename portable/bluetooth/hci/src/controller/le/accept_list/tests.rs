use bt_hci::cmd::{Opcode, OpcodeGroup};

use super::*;

fn decode(ocf: u16, parameters: &[u8]) -> Result<LeAcceptListCommand, LeAcceptListDecodeError> {
    LeAcceptListCommand::decode(HciCommandPacket::new(
        Opcode::new(OpcodeGroup::LE, ocf),
        parameters,
    ))
}

fn status(result: Result<LeAcceptListCommand, LeAcceptListDecodeError>) -> u8 {
    match result {
        Err(LeAcceptListDecodeError::Malformed(event)) => event.as_bytes()[5],
        other => panic!("not a malformed command: {other:?}"),
    }
}

#[test]
fn list_commands_decode_their_device() {
    assert_eq!(decode(0x10, &[]), Ok(LeAcceptListCommand::Clear));
    assert_eq!(
        decode(0x11, &[1, 1, 2, 3, 4, 5, 0xc6]),
        Ok(LeAcceptListCommand::Add(LeAcceptListEntry::Device(
            LeAcceptListDevice {
                random: true,
                address: [1, 2, 3, 4, 5, 0xc6],
            }
        )))
    );
    assert_eq!(
        decode(0x12, &[0, 6, 5, 4, 3, 2, 1]),
        Ok(LeAcceptListCommand::Remove(LeAcceptListEntry::Device(
            LeAcceptListDevice {
                random: false,
                address: [6, 5, 4, 3, 2, 1],
            }
        )))
    );
    assert_eq!(
        decode(0x12, &[0xff, 0, 0, 0, 0, 0, 0]),
        Ok(LeAcceptListCommand::Remove(LeAcceptListEntry::Anonymous))
    );
    assert_eq!(decode(0x13, &[]), Err(LeAcceptListDecodeError::Unsupported));
}

#[test]
fn malformed_commands_and_unknown_address_types_are_invalid_parameters() {
    assert_eq!(status(decode(0x10, &[0])), 0x12);
    assert_eq!(status(decode(0x11, &[0, 1, 2, 3])), 0x12);
    assert_eq!(status(decode(0x11, &[2, 1, 2, 3, 4, 5, 6])), 0x12);
}
