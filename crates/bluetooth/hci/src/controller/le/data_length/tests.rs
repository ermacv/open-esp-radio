use bt_hci::{
    FromHciBytes,
    cmd::{
        Cmd,
        le::{
            LeReadMaxDataLength, LeReadSuggestedDefaultDataLength, LeSetDataLength,
            LeWriteSuggestedDefaultDataLength,
        },
    },
    event::{CommandComplete, Event, EventPacket, le::LeEvent},
    param::{ConnHandle, Error as HciError, Status},
};

use super::{
    LeDataLengthChangeEvent, LeDataLengthCommand, LeDataLengthCommandCompleteEvent,
    LeDataLengthDecodeError, LeDataLengthParameters,
};
use crate::HciCommandPacket;

const MAXIMUM: LeDataLengthParameters = LeDataLengthParameters {
    octets: 251,
    time_micros: 2120,
};

fn decode(
    opcode: bt_hci::cmd::Opcode,
    parameters: &[u8],
) -> Result<LeDataLengthCommand, LeDataLengthDecodeError> {
    LeDataLengthCommand::decode(HciCommandPacket::new(opcode, parameters))
}

fn invalid(result: Result<LeDataLengthCommand, LeDataLengthDecodeError>) -> Status {
    match result {
        Err(LeDataLengthDecodeError::Malformed(response)) => {
            let Ok((Event::CommandComplete(complete), _)) =
                Event::from_hci_bytes(response.as_bytes())
            else {
                panic!("a Command Complete");
            };
            Status::from(complete.bytes[0])
        }
        other => panic!("expected a malformed command: {other:?}"),
    }
}

#[test]
fn commands_decode_their_parameters_within_the_specified_ranges() {
    assert_eq!(
        decode(
            LeSetDataLength::OPCODE,
            &[0x01, 0x00, 0xfb, 0x00, 0x48, 0x08]
        ),
        Ok(LeDataLengthCommand::Set {
            handle: ConnHandle::new(1),
            transmit: MAXIMUM,
        })
    );
    assert_eq!(
        decode(
            LeWriteSuggestedDefaultDataLength::OPCODE,
            &[0x1b, 0x00, 0x48, 0x01]
        ),
        Ok(LeDataLengthCommand::WriteSuggestedDefault(
            LeDataLengthParameters {
                octets: 27,
                time_micros: 328,
            }
        ))
    );
    assert_eq!(
        decode(LeReadSuggestedDefaultDataLength::OPCODE, &[]),
        Ok(LeDataLengthCommand::ReadSuggestedDefault)
    );
    assert_eq!(
        decode(LeReadMaxDataLength::OPCODE, &[]),
        Ok(LeDataLengthCommand::ReadMaximum)
    );
    assert_eq!(
        decode(bt_hci::cmd::le::LeRand::OPCODE, &[]),
        Err(LeDataLengthDecodeError::Unsupported)
    );
}

#[test]
fn out_of_range_or_truncated_parameters_are_invalid() {
    let status = HciError::INVALID_HCI_PARAMETERS.to_status();
    for parameters in [
        &[0x01, 0x00, 0x1a, 0x00, 0x48, 0x01][..],
        &[0x01, 0x00, 0xfc, 0x00, 0x48, 0x08][..],
        &[0x01, 0x00, 0xfb, 0x00, 0x47, 0x01][..],
        &[0x01, 0x00, 0xfb, 0x00, 0x91, 0x42][..],
        &[0x00, 0x0f, 0xfb, 0x00, 0x48, 0x08][..],
        &[0x01, 0x00, 0xfb, 0x00, 0x48][..],
    ] {
        assert_eq!(invalid(decode(LeSetDataLength::OPCODE, parameters)), status);
    }
    assert_eq!(invalid(decode(LeReadMaxDataLength::OPCODE, &[0])), status);
    assert_eq!(
        invalid(decode(
            LeWriteSuggestedDefaultDataLength::OPCODE,
            &[0x1b, 0x00]
        )),
        status
    );
}

#[test]
fn completions_carry_the_standard_return_parameters() {
    let complete = |event: LeDataLengthCommandCompleteEvent| {
        let bytes = event.as_bytes().to_vec();
        let Ok((Event::CommandComplete(complete), _)) = Event::from_hci_bytes(&bytes) else {
            panic!("a Command Complete");
        };
        let complete: CommandComplete<'_> = complete;
        (
            complete.cmd_opcode,
            Status::from(complete.bytes[0]),
            complete.bytes[1..].to_vec(),
        )
    };
    assert_eq!(
        complete(LeDataLengthCommandCompleteEvent::maximum(MAXIMUM, MAXIMUM)),
        (
            LeReadMaxDataLength::OPCODE,
            Status::SUCCESS,
            [0xfb, 0, 0x48, 8, 0xfb, 0, 0x48, 8].to_vec()
        )
    );
    assert_eq!(
        complete(LeDataLengthCommandCompleteEvent::set(
            Status::SUCCESS,
            ConnHandle::new(1)
        )),
        (LeSetDataLength::OPCODE, Status::SUCCESS, [1, 0].to_vec())
    );
    assert_eq!(
        complete(LeDataLengthCommandCompleteEvent::suggested_default_written(
            Status::SUCCESS
        )),
        (
            LeWriteSuggestedDefaultDataLength::OPCODE,
            Status::SUCCESS,
            [].to_vec()
        )
    );
}

#[test]
fn the_change_event_decodes_as_le_data_length_change() {
    let event = LeDataLengthChangeEvent::new(
        ConnHandle::new(0),
        MAXIMUM,
        LeDataLengthParameters {
            octets: 27,
            time_micros: 328,
        },
    );
    let packet = EventPacket::from_hci_bytes_complete(event.as_bytes()).unwrap();
    let Ok(Event::Le(LeEvent::LeDataLengthChange(change))) = Event::try_from(packet) else {
        panic!("an LE Data Length Change");
    };
    assert_eq!(change.handle, ConnHandle::new(0));
    assert_eq!(change.max_tx_octets, 251);
    assert_eq!(change.max_tx_time, 2120);
    assert_eq!(change.max_rx_octets, 27);
    assert_eq!(change.max_rx_time, 328);
}
