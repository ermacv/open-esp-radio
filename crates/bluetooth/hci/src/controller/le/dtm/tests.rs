use bt_hci::{
    ControllerToHostPacket, FromHciBytes, PacketKind,
    cmd::{
        Cmd, Opcode,
        le::{LeReadSupportedStates, LeReceiverTestV2, LeTestEnd, LeTransmitterTestV2},
    },
    event::{CommandComplete, CommandCompleteWithStatus, EventKind},
    param::{Error as HciError, Status},
};

use crate::test_support::command_packet;

use super::{
    LE_RECEIVER_TEST_V1_OPCODE, LE_RECEIVER_TEST_V2_OPCODE, LE_TEST_END_OPCODE,
    LE_TRANSMITTER_TEST_V1_OPCODE, LE_TRANSMITTER_TEST_V2_OPCODE, LeDtmActiveSessionDisposition,
    LeDtmCommand, LeDtmCommandDecodeError, LeDtmCommandKind, LeDtmIdleSessionDisposition,
    LeDtmModulationIndex, LeDtmPayloadPattern, LeDtmPhy,
};

#[test]
fn legacy_transmitter_opcode_does_not_collide_with_read_supported_states() {
    assert_eq!(LE_TRANSMITTER_TEST_V1_OPCODE.to_raw(), 0x201e);
    assert_ne!(LE_TRANSMITTER_TEST_V1_OPCODE, LeReadSupportedStates::OPCODE);
}

#[test]
fn all_command_bodies_decode_semantically_and_typed_test_end_crosses_the_boundary() {
    let receiver = LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[39]).unwrap();
    let LeDtmCommand::ReceiverTest(receiver) = receiver else {
        panic!("receiver command changed semantic kind");
    };
    assert_eq!(receiver.channel().index(), 39);

    let transmitter =
        LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V1_OPCODE, &[7, 255, 6]).unwrap();
    let LeDtmCommand::TransmitterTest(transmitter) = transmitter else {
        panic!("transmitter command changed semantic kind");
    };
    assert_eq!(transmitter.channel().index(), 7);
    assert_eq!(transmitter.payload_length(), 255);
    assert_eq!(
        transmitter.payload_pattern(),
        LeDtmPayloadPattern::Repeated00001111
    );

    let LeDtmCommand::ReceiverTest(receiver) = cross_hci_boundary(&LeReceiverTestV2::new(12, 3, 1))
    else {
        panic!("enhanced receiver command changed semantic kind");
    };
    assert_eq!(receiver.opcode(), LE_RECEIVER_TEST_V2_OPCODE);
    assert_eq!(receiver.phy(), LeDtmPhy::LeCoded);
    assert_eq!(receiver.modulation_index(), LeDtmModulationIndex::Stable);

    let LeDtmCommand::TransmitterTest(transmitter) =
        cross_hci_boundary(&LeTransmitterTestV2::new(18, 255, 4, 4))
    else {
        panic!("enhanced transmitter command changed semantic kind");
    };
    assert_eq!(transmitter.opcode(), LE_TRANSMITTER_TEST_V2_OPCODE);
    assert_eq!(transmitter.phy(), LeDtmPhy::LeCodedS2);

    let test_end = cross_hci_boundary(&LeTestEnd::new());
    assert!(matches!(test_end, LeDtmCommand::TestEnd(_)));
}

#[test]
fn v2_phy_and_modulation_domains_normalize_without_version_specific_tokens() {
    for (selector, expected) in [
        (1, LeDtmPhy::Le1M),
        (2, LeDtmPhy::Le2M),
        (3, LeDtmPhy::LeCoded),
    ] {
        for (parameter, modulation) in [
            (0, LeDtmModulationIndex::Standard),
            (1, LeDtmModulationIndex::Stable),
        ] {
            let LeDtmCommand::ReceiverTest(command) =
                LeDtmCommand::decode_body(LE_RECEIVER_TEST_V2_OPCODE, &[39, selector, parameter])
                    .expect("the standard receiver mode must normalize")
            else {
                unreachable!()
            };
            assert_eq!(command.phy(), expected);
            assert_eq!(command.modulation_index(), modulation);
        }
    }

    for (selector, expected) in [
        (1, LeDtmPhy::Le1M),
        (2, LeDtmPhy::Le2M),
        (3, LeDtmPhy::LeCoded),
        (4, LeDtmPhy::LeCodedS2),
    ] {
        let LeDtmCommand::TransmitterTest(command) =
            LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V2_OPCODE, &[39, 255, 7, selector])
                .expect("the standard transmitter mode must normalize")
        else {
            unreachable!()
        };
        assert_eq!(command.phy(), expected);
    }
}

#[test]
fn exact_parameter_lengths_fail_closed_without_partial_decode() {
    for (opcode, parameters, command, expected) in [
        (
            LE_RECEIVER_TEST_V1_OPCODE,
            &[][..],
            LeDtmCommandKind::ReceiverTestV1,
            1,
        ),
        (
            LE_RECEIVER_TEST_V1_OPCODE,
            &[0, 1][..],
            LeDtmCommandKind::ReceiverTestV1,
            1,
        ),
        (
            LE_TRANSMITTER_TEST_V1_OPCODE,
            &[0, 0][..],
            LeDtmCommandKind::TransmitterTestV1,
            3,
        ),
        (
            LE_RECEIVER_TEST_V2_OPCODE,
            &[0, 1][..],
            LeDtmCommandKind::ReceiverTestV2,
            3,
        ),
        (
            LE_RECEIVER_TEST_V2_OPCODE,
            &[0, 1, 0, 0][..],
            LeDtmCommandKind::ReceiverTestV2,
            3,
        ),
        (
            LE_TRANSMITTER_TEST_V2_OPCODE,
            &[0, 0, 0][..],
            LeDtmCommandKind::TransmitterTestV2,
            4,
        ),
        (
            LE_TRANSMITTER_TEST_V2_OPCODE,
            &[0, 0, 0, 1, 0][..],
            LeDtmCommandKind::TransmitterTestV2,
            4,
        ),
        (
            LE_TRANSMITTER_TEST_V1_OPCODE,
            &[0, 0, 0, 0][..],
            LeDtmCommandKind::TransmitterTestV1,
            3,
        ),
        (LE_TEST_END_OPCODE, &[0][..], LeDtmCommandKind::TestEnd, 0),
    ] {
        assert_eq!(
            LeDtmCommand::decode_body(opcode, parameters),
            Err(LeDtmCommandDecodeError::InvalidParameterLength {
                command,
                expected,
                actual: parameters.len(),
            })
        );
    }
}

#[test]
fn reserved_semantic_values_are_not_forwarded_to_a_chip() {
    assert_eq!(
        LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[40]),
        Err(LeDtmCommandDecodeError::ChannelOutsideTestDomain {
            command: LeDtmCommandKind::ReceiverTestV1,
            channel: 40,
        })
    );
    assert_eq!(
        LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V1_OPCODE, &[0, 1, 8]),
        Err(LeDtmCommandDecodeError::UnsupportedPayloadPattern {
            command: LeDtmCommandKind::TransmitterTestV1,
            selector: 8,
        })
    );
    assert_eq!(
        LeDtmCommand::decode_body(LE_RECEIVER_TEST_V2_OPCODE, &[0, 4, 0]),
        Err(LeDtmCommandDecodeError::UnsupportedPhy {
            command: LeDtmCommandKind::ReceiverTestV2,
            selector: 4,
        })
    );
    assert!(matches!(
        LeDtmCommand::decode_body(LE_RECEIVER_TEST_V2_OPCODE, &[0, 0, 0]),
        Err(LeDtmCommandDecodeError::UnsupportedPhy {
            command: LeDtmCommandKind::ReceiverTestV2,
            selector: 0,
        })
    ));
    assert_eq!(
        LeDtmCommand::decode_body(LE_RECEIVER_TEST_V2_OPCODE, &[0, 1, 2]),
        Err(LeDtmCommandDecodeError::UnsupportedModulationIndex {
            command: LeDtmCommandKind::ReceiverTestV2,
            parameter: 2,
        })
    );
    assert_eq!(
        LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V2_OPCODE, &[0, 1, 0, 5]),
        Err(LeDtmCommandDecodeError::UnsupportedPhy {
            command: LeDtmCommandKind::TransmitterTestV2,
            selector: 5,
        })
    );
    assert!(matches!(
        LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V2_OPCODE, &[0, 1, 0, 0]),
        Err(LeDtmCommandDecodeError::UnsupportedPhy {
            command: LeDtmCommandKind::TransmitterTestV2,
            selector: 0,
        })
    ));
    assert!(matches!(
        LeDtmCommand::decode_body(Opcode::UNSOLICITED, &[]),
        Err(LeDtmCommandDecodeError::UnsupportedOpcode { .. })
    ));
}

#[test]
fn rejected_known_opcodes_build_specified_command_completions() {
    for (opcode, parameters) in [
        (LE_RECEIVER_TEST_V1_OPCODE, &[][..]),
        (LE_RECEIVER_TEST_V1_OPCODE, &[40][..]),
        (LE_TRANSMITTER_TEST_V1_OPCODE, &[0, 1, 8][..]),
        (LE_RECEIVER_TEST_V2_OPCODE, &[0, 1, 2][..]),
        (LE_TEST_END_OPCODE, &[0][..]),
    ] {
        let response = LeDtmCommand::decode_body(opcode, parameters)
            .expect_err("malformed known command must fail closed")
            .into_command_complete()
            .expect("known DTM opcode must retain its response identity");
        let observed = parse_command_complete(response.as_bytes());
        assert_eq!(observed.cmd_opcode, opcode);
        assert_eq!(
            observed.status,
            HciError::INVALID_HCI_PARAMETERS.to_status()
        );
        assert!(observed.return_param_bytes.is_empty());
    }

    for (opcode, parameters) in [
        (LE_RECEIVER_TEST_V2_OPCODE, &[0, 4, 0][..]),
        (LE_TRANSMITTER_TEST_V2_OPCODE, &[0, 1, 0, 5][..]),
    ] {
        let response = LeDtmCommand::decode_body(opcode, parameters)
            .expect_err("a reserved PHY must fail closed")
            .into_command_complete()
            .expect("the known DTM opcode retains response identity");
        let observed = parse_command_complete(response.as_bytes());
        assert_eq!(observed.cmd_opcode, opcode);
        assert_eq!(observed.status, HciError::UNSUPPORTED.to_status());
    }

    let unsupported = LeDtmCommand::decode_body(Opcode::UNSOLICITED, &[])
        .expect_err("unsupported opcode remains outside the DTM response scope");
    assert_eq!(
        unsupported.into_command_complete(),
        Err(LeDtmCommandDecodeError::UnsupportedOpcode {
            opcode: Opcode::UNSOLICITED,
        })
    );
}

#[test]
fn successful_starts_roundtrip_through_bt_hci_event_types() {
    let LeDtmCommand::ReceiverTest(receiver) =
        LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[3]).unwrap()
    else {
        unreachable!()
    };
    let receiver_complete = receiver.into_started_command_complete();
    let observed = parse_command_complete(receiver_complete.as_bytes());
    assert_eq!(observed.cmd_opcode, LE_RECEIVER_TEST_V1_OPCODE);
    assert_eq!(observed.status, Status::SUCCESS);
    assert!(observed.return_param_bytes.is_empty());

    let LeDtmCommand::TransmitterTest(transmitter) =
        LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V1_OPCODE, &[5, 37, 2]).unwrap()
    else {
        unreachable!()
    };
    let transmitter_complete = transmitter.into_started_command_complete();
    let observed = parse_command_complete(transmitter_complete.as_bytes());
    assert_eq!(observed.cmd_opcode, LE_TRANSMITTER_TEST_V1_OPCODE);
    assert_eq!(observed.status, Status::SUCCESS);
    assert!(observed.return_param_bytes.is_empty());

    let LeDtmCommand::ReceiverTest(receiver) =
        LeDtmCommand::decode_body(LE_RECEIVER_TEST_V2_OPCODE, &[3, 2, 0]).unwrap()
    else {
        unreachable!()
    };
    let receiver_complete = receiver.into_started_command_complete();
    let observed = parse_command_complete(receiver_complete.as_bytes());
    assert_eq!(observed.cmd_opcode, LE_RECEIVER_TEST_V2_OPCODE);
    assert_eq!(observed.status, Status::SUCCESS);

    let LeDtmCommand::TransmitterTest(transmitter) =
        LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V2_OPCODE, &[5, 37, 2, 3]).unwrap()
    else {
        unreachable!()
    };
    let transmitter_complete = transmitter.into_started_command_complete();
    let observed = parse_command_complete(transmitter_complete.as_bytes());
    assert_eq!(observed.cmd_opcode, LE_TRANSMITTER_TEST_V2_OPCODE);
    assert_eq!(observed.status, Status::SUCCESS);
}

#[test]
fn idle_policy_retains_starts_and_completes_empty_test_end() {
    let receiver = LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[17]).unwrap();
    let LeDtmIdleSessionDisposition::StartReceiver(receiver) =
        receiver.into_idle_session_disposition()
    else {
        panic!("an idle receiver start did not retain its command")
    };
    assert_eq!(receiver.channel().index(), 17);

    let transmitter =
        LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V1_OPCODE, &[9, 31, 2]).unwrap();
    let LeDtmIdleSessionDisposition::StartTransmitter(transmitter) =
        transmitter.into_idle_session_disposition()
    else {
        panic!("an idle transmitter start did not retain its command")
    };
    assert_eq!(transmitter.channel().index(), 9);
    assert_eq!(transmitter.payload_length(), 31);

    let test_end = cross_hci_boundary(&LeTestEnd::new());
    let LeDtmIdleSessionDisposition::CompleteNoTest(response) =
        test_end.into_idle_session_disposition()
    else {
        panic!("idle Test End did not produce its terminal response")
    };
    let observed = parse_command_complete(response.as_bytes());
    assert_eq!(observed.cmd_opcode, LeTestEnd::OPCODE);
    assert_eq!(observed.status, Status::SUCCESS);
    assert_eq!(observed.return_params::<LeTestEnd>().unwrap(), 0);
}

#[test]
fn active_policy_rejects_both_starts_as_controller_busy() {
    for (command, expected_opcode) in [
        (
            LeDtmCommand::decode_body(LE_RECEIVER_TEST_V1_OPCODE, &[3]).unwrap(),
            LE_RECEIVER_TEST_V1_OPCODE,
        ),
        (
            LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V1_OPCODE, &[5, 37, 2]).unwrap(),
            LE_TRANSMITTER_TEST_V1_OPCODE,
        ),
        (
            LeDtmCommand::decode_body(LE_RECEIVER_TEST_V2_OPCODE, &[3, 2, 0]).unwrap(),
            LE_RECEIVER_TEST_V2_OPCODE,
        ),
        (
            LeDtmCommand::decode_body(LE_TRANSMITTER_TEST_V2_OPCODE, &[5, 37, 2, 4]).unwrap(),
            LE_TRANSMITTER_TEST_V2_OPCODE,
        ),
    ] {
        let LeDtmActiveSessionDisposition::RejectControllerBusy(response) =
            command.into_active_session_disposition()
        else {
            panic!("a second active-session start escaped busy rejection")
        };
        let observed = parse_command_complete(response.as_bytes());
        assert_eq!(observed.cmd_opcode, expected_opcode);
        assert_eq!(observed.status, HciError::CONTROLLER_BUSY.to_status());
        assert!(observed.return_param_bytes.is_empty());
    }
}

#[test]
fn active_test_end_stays_owned_until_terminal_count_is_available() {
    let command = cross_hci_boundary(&LeTestEnd::new());
    let LeDtmActiveSessionDisposition::End(command) = command.into_active_session_disposition()
    else {
        panic!("active Test End was completed before hardware quiescence")
    };

    let response = command.into_ended_command_complete(0x3412);
    let observed = parse_command_complete(response.as_bytes());
    assert_eq!(observed.status, Status::SUCCESS);
    assert_eq!(observed.return_params::<LeTestEnd>().unwrap(), 0x3412);
}

fn cross_hci_boundary<T: bt_hci::transport::PacketToController>(command: &T) -> LeDtmCommand {
    let mut buffer = [0; 16];
    LeDtmCommand::decode(command_packet(command, &mut buffer)).unwrap()
}

fn parse_command_complete(bytes: &[u8]) -> CommandCompleteWithStatus<'_> {
    let (packet, remaining) =
        ControllerToHostPacket::from_hci_bytes_with_kind(PacketKind::Event, bytes).unwrap();
    assert!(remaining.is_empty());
    let ControllerToHostPacket::Event(event) = packet else {
        panic!("Command Complete changed packet class");
    };
    assert_eq!(event.kind, EventKind::CommandComplete);
    let complete = CommandComplete::from_hci_bytes_complete(event.data).unwrap();
    complete.try_into().unwrap()
}
