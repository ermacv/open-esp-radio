use bt_hci::{
    cmd::{
        Cmd, Opcode, OpcodeGroup,
        controller_baseband::{Reset, SetEventMask},
        le::{
            LeLongTermKeyRequestNegativeReply, LeLongTermKeyRequestReply, LeReadRemoteFeatures,
            LeSetAdvData, LeSetAdvEnable, LeSetAdvParams, LeSetRandomAddr, LeSetScanResponseData,
        },
        link_control::{Disconnect, ReadRemoteVersionInformation},
    },
    param::{BdAddr, Error as HciError, EventMask, Status},
};

use super::{LeControllerCommandClassification, classify_le_controller_command};
use crate::{
    BluetoothPublicDeviceAddress, BootstrapCommand, BootstrapPhase, HciCommandPacket,
    LE_RECEIVER_TEST_V1_OPCODE, LE_RECEIVER_TEST_V2_OPCODE, LE_TEST_END_OPCODE,
    LE_TRANSMITTER_TEST_V1_OPCODE, LE_TRANSMITTER_TEST_V2_OPCODE, LeControllerBootstrap,
    LeControllerBootstrapConfig, LeDisconnectCommand, LeDtmCommand,
    LeLegacyAdvertisingConfigurationCommand, OwnedBootstrapCommand,
};

#[test]
fn disconnect_is_owned_and_malformed_input_keeps_command_status_semantics() {
    let classified =
        classify_le_controller_command(HciCommandPacket::new(Disconnect::OPCODE, &[1, 0, 0x13]));
    let LeControllerCommandClassification::Disconnect(command) = classified else {
        panic!("valid Disconnect did not become a semantic command");
    };
    assert_eq!(command.handle().raw(), 1);
    assert_eq!(command.reason(), 0x13);

    let malformed = classify_le_controller_command(HciCommandPacket::new(
        LeDisconnectCommand::OPCODE,
        &[1, 0, 0x16],
    ));
    let LeControllerCommandClassification::MalformedDisconnect(response) = malformed else {
        panic!("invalid reason escaped the Disconnect command family");
    };
    assert_eq!(
        response.status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
}

#[test]
fn read_remote_features_is_classified_before_the_closed_bootstrap_table() {
    let classified = classify_le_controller_command(HciCommandPacket::new(
        LeReadRemoteFeatures::OPCODE,
        &[1, 0],
    ));
    let LeControllerCommandClassification::ReadRemoteFeatures(command) = classified else {
        panic!("valid remote-feature request did not retain its semantic command");
    };
    assert_eq!(command.handle(), bt_hci::param::ConnHandle::new(1));

    let malformed =
        classify_le_controller_command(HciCommandPacket::new(LeReadRemoteFeatures::OPCODE, &[1]));
    let LeControllerCommandClassification::MalformedReadRemoteFeatures(response) = malformed else {
        panic!("malformed remote-feature request escaped its command family");
    };
    assert_eq!(
        response.status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
}

#[test]
fn read_remote_version_is_classified_before_the_closed_bootstrap_table() {
    let classified = classify_le_controller_command(HciCommandPacket::new(
        ReadRemoteVersionInformation::OPCODE,
        &[1, 0],
    ));
    let LeControllerCommandClassification::ReadRemoteVersionInformation(command) = classified
    else {
        panic!("valid remote-version request did not retain its semantic command");
    };
    assert_eq!(command.handle(), bt_hci::param::ConnHandle::new(1));

    let malformed = classify_le_controller_command(HciCommandPacket::new(
        ReadRemoteVersionInformation::OPCODE,
        &[1],
    ));
    let LeControllerCommandClassification::MalformedReadRemoteVersionInformation(response) =
        malformed
    else {
        panic!("malformed remote-version request escaped its command family");
    };
    assert_eq!(
        response.status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
}

#[test]
fn long_term_key_replies_are_owned_before_the_closed_bootstrap_table() {
    let mut parameters = [0; 18];
    parameters[..2].copy_from_slice(&1_u16.to_le_bytes());
    parameters[2..].copy_from_slice(&[0x5a; 16]);
    let classified = classify_le_controller_command(HciCommandPacket::new(
        LeLongTermKeyRequestReply::OPCODE,
        &parameters,
    ));
    let LeControllerCommandClassification::LongTermKeyReply(command) = classified else {
        panic!("valid positive LTK reply escaped its command family");
    };
    assert_eq!(command.handle(), bt_hci::param::ConnHandle::new(1));
    assert_eq!(command.into_long_term_key(), [0x5a; 16]);

    let classified = classify_le_controller_command(HciCommandPacket::new(
        LeLongTermKeyRequestNegativeReply::OPCODE,
        &[1, 0],
    ));
    let LeControllerCommandClassification::LongTermKeyNegativeReply(command) = classified else {
        panic!("valid negative LTK reply escaped its command family");
    };
    assert_eq!(command.handle(), bt_hci::param::ConnHandle::new(1));

    let malformed = classify_le_controller_command(HciCommandPacket::new(
        LeLongTermKeyRequestReply::OPCODE,
        &[1, 0],
    ));
    let LeControllerCommandClassification::MalformedLongTermKeyReply(response) = malformed else {
        panic!("malformed LTK reply escaped its command family");
    };
    assert_eq!(
        response.status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
}

#[test]
fn bootstrap_command_is_owned_without_advancing_software_state() {
    let mut bootstrap = bootstrap();
    let classified = classify_le_controller_command(HciCommandPacket::new(Reset::OPCODE, &[]));

    let LeControllerCommandClassification::Bootstrap(command) = classified else {
        panic!("Reset did not become an owned bootstrap command");
    };
    assert_eq!(command.kind(), BootstrapCommand::Reset);
    assert!(command.is_reset());
    assert_eq!(bootstrap.phase(), BootstrapPhase::AwaitingReset);

    let response = bootstrap.dispatch(command, false);
    assert_eq!(response.status(), Status::SUCCESS);
    assert_eq!(bootstrap.phase(), BootstrapPhase::Configuring);
}

#[test]
fn bootstrap_payload_is_typed_and_independent_of_receive_storage() {
    let mut parameters = [6, 5, 4, 3, 2, 0xc1];
    let command = match classify_le_controller_command(HciCommandPacket::new(
        LeSetRandomAddr::OPCODE,
        &parameters,
    )) {
        LeControllerCommandClassification::Bootstrap(command) => command,
        _ => panic!("random address did not become an owned bootstrap command"),
    };

    parameters.fill(0);
    let OwnedBootstrapCommand::LeSetRandomAddress(address) = command else {
        panic!("random address lost its semantic bootstrap variant");
    };
    assert_eq!(address, BdAddr::new([6, 5, 4, 3, 2, 0xc1]));
}

#[test]
fn active_reset_can_be_held_until_the_session_policy_dispatches_it() {
    let mut bootstrap = bootstrap();
    assert_eq!(
        bootstrap
            .dispatch(OwnedBootstrapCommand::Reset, false)
            .status(),
        Status::SUCCESS
    );
    let requested_mask = EventMask::new().enable_hardware_error(true);
    assert_eq!(
        bootstrap
            .dispatch(OwnedBootstrapCommand::SetEventMask(requested_mask), false)
            .status(),
        Status::SUCCESS
    );

    let classified = classify_le_controller_command(HciCommandPacket::new(Reset::OPCODE, &[]));
    assert_eq!(bootstrap.event_mask(), requested_mask);
    let LeControllerCommandClassification::Bootstrap(reset) = classified else {
        panic!("active Reset did not remain an owned policy input");
    };
    assert!(reset.is_reset());
    assert_eq!(bootstrap.event_mask(), requested_mask);

    assert_eq!(bootstrap.dispatch(reset, false).status(), Status::SUCCESS);
    assert_eq!(
        bootstrap.event_mask(),
        crate::controller::bootstrap::state::default_event_mask()
    );
}

#[test]
fn malformed_known_bootstrap_is_owned_without_touching_an_epoch() {
    let mut bootstrap = bootstrap();
    let requested_mask = EventMask::new().enable_hardware_error(true);
    assert_eq!(
        bootstrap
            .dispatch(OwnedBootstrapCommand::Reset, false)
            .status(),
        Status::SUCCESS
    );
    assert_eq!(
        bootstrap
            .dispatch(OwnedBootstrapCommand::SetEventMask(requested_mask), false)
            .status(),
        Status::SUCCESS
    );

    let classified =
        classify_le_controller_command(HciCommandPacket::new(SetEventMask::OPCODE, &[0; 7]));

    let LeControllerCommandClassification::MalformedBootstrap(response) = classified else {
        panic!("malformed bootstrap command escaped its command family");
    };
    assert_eq!(response.opcode(), SetEventMask::OPCODE);
    assert_eq!(
        response.status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
    assert_eq!(bootstrap.phase(), BootstrapPhase::Configuring);
    assert_eq!(bootstrap.event_mask(), requested_mask);
}

#[test]
fn valid_dtm_command_becomes_an_owned_semantic_token() {
    let classified =
        classify_le_controller_command(HciCommandPacket::new(LE_RECEIVER_TEST_V1_OPCODE, &[39]));

    let LeControllerCommandClassification::Dtm(LeDtmCommand::ReceiverTest(command)) = classified
    else {
        panic!("valid receiver test did not become a semantic DTM command");
    };
    assert_eq!(command.channel().index(), 39);
}

#[test]
fn advertising_configuration_and_enable_are_owned() {
    let parameters = classify_le_controller_command(HciCommandPacket::new(
        LeSetAdvParams::OPCODE,
        &[
            0x20, 0x00, 0x40, 0x00, 0x03, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0x07, 0x00,
        ],
    ));
    assert!(matches!(
        parameters,
        LeControllerCommandClassification::LegacyAdvertisingConfiguration(
            LeLegacyAdvertisingConfigurationCommand::SetParameters(_)
        )
    ));

    let data =
        classify_le_controller_command(HciCommandPacket::new(LeSetAdvData::OPCODE, &[0; 32]));
    assert!(matches!(
        data,
        LeControllerCommandClassification::LegacyAdvertisingConfiguration(
            LeLegacyAdvertisingConfigurationCommand::SetData(_)
        )
    ));

    let scan_response = classify_le_controller_command(HciCommandPacket::new(
        LeSetScanResponseData::OPCODE,
        &[0; 32],
    ));
    assert!(matches!(
        scan_response,
        LeControllerCommandClassification::LegacyAdvertisingConfiguration(
            LeLegacyAdvertisingConfigurationCommand::SetScanResponseData(_)
        )
    ));

    let enable =
        classify_le_controller_command(HciCommandPacket::new(LeSetAdvEnable::OPCODE, &[1]));
    assert!(matches!(
        enable,
        LeControllerCommandClassification::LegacyAdvertisingEnable(_)
    ));
}

#[test]
fn malformed_claimed_advertising_configuration_has_exact_status() {
    for opcode in [LeSetAdvData::OPCODE, LeSetScanResponseData::OPCODE] {
        let classified = classify_le_controller_command(HciCommandPacket::new(opcode, &[32; 32]));
        let LeControllerCommandClassification::MalformedLegacyAdvertising(response) = classified
        else {
            panic!("invalid advertising data escaped its claimed family");
        };
        assert_eq!(response.opcode(), opcode);
        assert_eq!(
            response.status(),
            HciError::INVALID_HCI_PARAMETERS.to_status()
        );
    }
}

#[test]
fn known_dtm_rejections_retain_their_required_status() {
    for (opcode, parameters, status) in [
        (
            LE_RECEIVER_TEST_V1_OPCODE,
            &[][..],
            HciError::INVALID_HCI_PARAMETERS.to_status(),
        ),
        (
            LE_TRANSMITTER_TEST_V1_OPCODE,
            &[0, 1, 8][..],
            HciError::INVALID_HCI_PARAMETERS.to_status(),
        ),
        (
            LE_RECEIVER_TEST_V2_OPCODE,
            &[0, 4, 0][..],
            HciError::UNSUPPORTED.to_status(),
        ),
        (
            LE_TRANSMITTER_TEST_V2_OPCODE,
            &[0, 1, 0, 5][..],
            HciError::UNSUPPORTED.to_status(),
        ),
        (
            LE_TEST_END_OPCODE,
            &[0][..],
            HciError::INVALID_HCI_PARAMETERS.to_status(),
        ),
    ] {
        let classified = classify_le_controller_command(HciCommandPacket::new(opcode, parameters));
        let LeControllerCommandClassification::MalformedDtm(response) = classified else {
            panic!("malformed known DTM command escaped its command family");
        };
        assert_eq!(response.opcode(), opcode);
        assert_eq!(response.status(), status);
    }
}

#[test]
fn malformed_enable_response_is_owned_across_receive_storage_reuse() {
    let mut parameters = [2];
    let classified =
        classify_le_controller_command(HciCommandPacket::new(LeSetAdvEnable::OPCODE, &parameters));
    parameters.fill(0);

    assert_eq!(classified.opcode(), LeSetAdvEnable::OPCODE);
    let LeControllerCommandClassification::MalformedLegacyAdvertising(response) = classified else {
        panic!("malformed Enable did not produce its owned response");
    };
    assert_eq!(response.opcode(), LeSetAdvEnable::OPCODE);
    assert_eq!(
        response.status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
}

#[test]
fn unrelated_opcode_group_produces_an_exact_unknown_command_completion() {
    let opcode = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 7);
    let classified = classify_le_controller_command(HciCommandPacket::new(opcode, &[2, 3, 5]));

    let LeControllerCommandClassification::Unsupported(response) = classified else {
        panic!("unclaimed opcode did not produce an owned terminal response");
    };
    assert_eq!(response.opcode(), opcode);
    assert_eq!(response.status(), HciError::UNKNOWN_CMD.to_status());
    assert_eq!(response.as_bytes().len(), 6);
}

fn bootstrap() -> LeControllerBootstrap {
    LeControllerBootstrap::new(
        LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
            27,
            1,
        )
        .expect("nonzero test profile"),
    )
}
