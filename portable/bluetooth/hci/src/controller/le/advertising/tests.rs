use bt_hci::{
    cmd::{
        Cmd,
        le::{LeSetAdvData, LeSetAdvEnable, LeSetAdvParams, LeSetScanResponseData},
    },
    param::{
        AddrKind, AdvChannelMap, AdvFilterPolicy, AdvKind, BdAddr, Duration, Error as HciError,
    },
};

use super::{
    LEGACY_ADVERTISING_INTERVAL_DEFAULT, LeLegacyAdvertisingCommand,
    LeLegacyAdvertisingCommandKind, LeLegacyAdvertisingConfiguration,
    LeLegacyAdvertisingConfigurationCommand, LeLegacyAdvertisingEnableRequest,
    LeLegacyAdvertisingOwnAddressKind, LeLegacyAdvertisingRandomAddressMissing,
    LeLegacyAdvertisingRole,
};
use crate::{BluetoothPublicDeviceAddress, HciCommandPacket, LeLegacyAdvertisingAddress};

#[test]
fn decodes_supported_nonconnectable_parameters() {
    let command = LeLegacyAdvertisingCommand::decode(HciCommandPacket::new(
        LeSetAdvParams::OPCODE,
        &[
            0x20, 0x00, 0x40, 0x00, 0x03, 0x01, 0x00, 0, 0, 0, 0, 0, 0, 0x05, 0x00,
        ],
    ))
    .expect("the supported standard parameters decode");

    let LeLegacyAdvertisingCommand::SetParameters(parameters) = command else {
        panic!("parameters changed semantic command kind");
    };
    assert_eq!(parameters.interval().minimum_units_625_us(), 0x20);
    assert_eq!(parameters.interval().maximum_units_625_us(), 0x40);
    assert_eq!(parameters.role(), LeLegacyAdvertisingRole::Nonconnectable);
    assert_eq!(
        parameters.own_address_kind(),
        LeLegacyAdvertisingOwnAddressKind::Random
    );
    assert!(parameters.channels().channel_37());
    assert!(!parameters.channels().channel_38());
    assert!(parameters.channels().channel_39());
}

#[test]
fn advertising_and_scan_response_data_are_owned_and_length_bounded() {
    for opcode in [LeSetAdvData::OPCODE, LeSetScanResponseData::OPCODE] {
        let mut body = [0; 32];
        body[0] = 3;
        body[1..4].copy_from_slice(&[2, 1, 6]);
        let command = LeLegacyAdvertisingCommand::decode(HciCommandPacket::new(opcode, &body))
            .expect("the complete standard data command decodes");
        body.fill(0xff);
        match command {
            LeLegacyAdvertisingCommand::SetData(data) => {
                assert_eq!(opcode, LeSetAdvData::OPCODE);
                assert_eq!(data.as_bytes(), &[2, 1, 6]);
            }
            LeLegacyAdvertisingCommand::SetScanResponseData(data) => {
                assert_eq!(opcode, LeSetScanResponseData::OPCODE);
                assert_eq!(data.as_bytes(), &[2, 1, 6]);
            }
            _ => panic!("data changed semantic command kind"),
        }
    }
}

#[test]
fn rejects_malformed_invalid_and_unsupported_values_with_exact_status() {
    for (opcode, body, expected) in [
        (
            LeSetAdvEnable::OPCODE,
            &[2][..],
            HciError::INVALID_HCI_PARAMETERS.to_status(),
        ),
        (
            LeSetAdvParams::OPCODE,
            // A resolvable private own address needs privacy.
            &[0x20, 0, 0x40, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 7, 0][..],
            HciError::UNSUPPORTED.to_status(),
        ),
        (
            LeSetAdvParams::OPCODE,
            &[0x20, 0, 0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 1][..],
            HciError::UNSUPPORTED.to_status(),
        ),
        (
            LeSetAdvParams::OPCODE,
            &[0x20, 0, 0x40, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0][..],
            HciError::INVALID_HCI_PARAMETERS.to_status(),
        ),
    ] {
        let error = LeLegacyAdvertisingCommand::decode(HciCommandPacket::new(opcode, body))
            .expect_err("the rejected value cannot become a command token");
        let response = error
            .into_command_complete()
            .expect("the opcode belongs to this command family");
        assert_eq!(response.opcode(), opcode);
        assert_eq!(response.status(), expected);
    }
}

#[test]
fn directed_roles_take_their_peer_and_reject_other_peer_kinds() {
    let mut body = [0; 15];
    // High duty ignores the invalid zero interval.
    body[4] = 1;
    body[6] = 1; // random peer
    body[7..13].copy_from_slice(&[1, 2, 3, 4, 5, 0xc6]);
    body[13] = 0x07;
    let Ok(LeLegacyAdvertisingCommand::SetParameters(parameters)) =
        LeLegacyAdvertisingCommand::decode(HciCommandPacket::new(LeSetAdvParams::OPCODE, &body))
    else {
        panic!("high duty directed parameters decode");
    };
    assert_eq!(parameters.role(), LeLegacyAdvertisingRole::DirectedHighDuty);
    assert!(parameters.peer().is_random());
    assert_eq!(
        parameters.peer().address(),
        BdAddr::new([1, 2, 3, 4, 5, 0xc6])
    );

    body[4] = 4;
    body[..4].copy_from_slice(&[0x20, 0, 0x40, 0]);
    body[6] = 2; // a resolvable peer identity needs privacy
    let error =
        LeLegacyAdvertisingCommand::decode(HciCommandPacket::new(LeSetAdvParams::OPCODE, &body))
            .expect_err("a directed peer must be public or random");
    assert_eq!(
        error
            .into_command_complete()
            .expect("Set Advertising Parameters owns the rejection")
            .status(),
        HciError::INVALID_HCI_PARAMETERS.to_status()
    );
}

#[test]
fn rejects_every_filtered_parameter_profile() {
    for unsupported_filter_policy in [1, 2, 3] {
        let mut body = [0; 15];
        body[..4].copy_from_slice(&[0x20, 0, 0x40, 0]);
        body[4] = 0;
        body[13] = 0x07;
        body[14] = unsupported_filter_policy;
        let error = LeLegacyAdvertisingCommand::decode(HciCommandPacket::new(
            LeSetAdvParams::OPCODE,
            &body,
        ))
        .expect_err("every filtered advertising profile remains unsupported");
        assert_eq!(
            error
                .into_command_complete()
                .expect("Set Advertising Parameters owns the rejection")
                .status(),
            HciError::UNSUPPORTED.to_status()
        );
    }
}

#[test]
fn accepts_standard_bt_hci_field_domains_without_reencoding_them() {
    let command = LeSetAdvParams::new(
        Duration::from_u16(0x20),
        Duration::from_u16(0x40),
        AdvKind::AdvNonconnInd,
        AddrKind::PUBLIC,
        AddrKind::PUBLIC,
        BdAddr::default(),
        AdvChannelMap::ALL,
        AdvFilterPolicy::Unfiltered,
    );
    let _ = command;
    assert_eq!(
        LeSetAdvParams::OPCODE,
        LeLegacyAdvertisingCommandKind::SetParameters.opcode()
    );
}

fn configuration(command: LeLegacyAdvertisingCommand) -> LeLegacyAdvertisingConfigurationCommand {
    LeLegacyAdvertisingConfigurationCommand::from_command(command)
        .expect("the fixture is a configuration command")
}

fn data_command(opcode: bt_hci::cmd::Opcode, data: &[u8]) -> LeLegacyAdvertisingCommand {
    let mut body = [0; 32];
    body[0] = data.len() as u8;
    body[1..=data.len()].copy_from_slice(data);
    LeLegacyAdvertisingCommand::decode(HciCommandPacket::new(opcode, &body))
        .expect("fixture data decode")
}

#[test]
fn configuration_starts_from_the_standard_defaults_and_applies_commands() {
    let parameters = LeLegacyAdvertisingCommand::decode(HciCommandPacket::new(
        LeSetAdvParams::OPCODE,
        &[
            0x20, 0x00, 0x40, 0x00, 0x03, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0x07, 0x00,
        ],
    ))
    .expect("fixture parameters decode");
    let mut state = LeLegacyAdvertisingConfiguration::new();
    let defaults = state.parameters();
    assert_eq!(defaults.role(), LeLegacyAdvertisingRole::Connectable);
    assert_eq!(
        defaults.own_address_kind(),
        LeLegacyAdvertisingOwnAddressKind::Public
    );
    assert!(defaults.channels().channel_37());
    assert!(defaults.channels().channel_38());
    assert!(defaults.channels().channel_39());
    assert_eq!(
        defaults.interval().minimum_units_625_us(),
        LEGACY_ADVERTISING_INTERVAL_DEFAULT
    );

    state.configure(configuration(parameters));
    assert_ne!(state.parameters(), defaults);
    state.configure(configuration(data_command(
        LeSetAdvData::OPCODE,
        &[2, 1, 6],
    )));
    assert_eq!(state.data().as_bytes(), &[2, 1, 6]);
    state.configure(configuration(data_command(
        LeSetScanResponseData::OPCODE,
        &[2, 1, 6],
    )));
    assert_eq!(state.scan_response_data().as_bytes(), &[2, 1, 6]);
    assert_eq!(
        LeLegacyAdvertisingConfiguration::default(),
        LeLegacyAdvertisingConfiguration::new()
    );
}

#[test]
fn enable_freezes_parameters_data_and_resolved_public_address() {
    let parameters = LeLegacyAdvertisingCommand::decode(HciCommandPacket::new(
        LeSetAdvParams::OPCODE,
        &[
            0x20, 0x00, 0x40, 0x00, 0x03, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0x05, 0x00,
        ],
    ))
    .expect("fixture parameters decode");
    let mut state = LeLegacyAdvertisingConfiguration::new();
    state.configure(configuration(parameters));
    state.configure(configuration(data_command(
        LeSetAdvData::OPCODE,
        &[2, 1, 6],
    )));
    let public_address = BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]);
    let Ok(LeLegacyAdvertisingEnableRequest::Nonconnectable(request)) =
        state.enable_request(public_address, None)
    else {
        panic!("ADV_NONCONN_IND parameters start a nonconnectable set");
    };
    assert_eq!(request.data().as_bytes(), &[2, 1, 6]);
    assert_eq!(
        request.advertiser(),
        LeLegacyAdvertisingAddress::Public(public_address)
    );
    assert_eq!(request.parameters().interval().minimum_units_625_us(), 0x20);
    assert_eq!(request.parameters().interval().maximum_units_625_us(), 0x40);
    assert!(request.parameters().channels().channel_37());
    assert!(!request.parameters().channels().channel_38());
    assert!(request.parameters().channels().channel_39());
}

#[test]
fn scannable_parameters_enable_a_response_capable_scannable_set() {
    let mut body = [0; 15];
    body[..4].copy_from_slice(&[0x20, 0, 0x40, 0]);
    body[4] = 2; // ADV_SCAN_IND
    body[13] = 0x07;
    let parameters =
        LeLegacyAdvertisingCommand::decode(HciCommandPacket::new(LeSetAdvParams::OPCODE, &body))
            .expect("scannable parameters decode");
    let mut state = LeLegacyAdvertisingConfiguration::new();
    state.configure(configuration(parameters));
    assert_eq!(
        state.parameters().role(),
        LeLegacyAdvertisingRole::Scannable
    );
    state.configure(configuration(data_command(
        LeSetScanResponseData::OPCODE,
        &[2, 1, 6],
    )));
    let public_address = BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]);
    let Ok(LeLegacyAdvertisingEnableRequest::Scannable(request)) =
        state.enable_request(public_address, None)
    else {
        panic!("ADV_SCAN_IND parameters start a scannable set");
    };
    assert_eq!(request.scan_response_data().as_bytes(), &[2, 1, 6]);
}

#[test]
fn connectable_enable_retains_scan_response_and_requires_a_random_address() {
    let public_address = BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]);
    let mut state = LeLegacyAdvertisingConfiguration::new();
    state.configure(configuration(data_command(
        LeSetScanResponseData::OPCODE,
        &[3, 3, 0xaa, 0xfe],
    )));
    let Ok(LeLegacyAdvertisingEnableRequest::Connectable(request)) =
        state.enable_request(public_address, None)
    else {
        panic!("the defaults start connectable undirected advertising");
    };
    assert!(request.data().is_empty());
    assert_eq!(request.scan_response_data().as_bytes(), &[3, 3, 0xaa, 0xfe]);

    let parameters = LeLegacyAdvertisingCommand::decode(HciCommandPacket::new(
        LeSetAdvParams::OPCODE,
        &[
            0x20, 0x00, 0x40, 0x00, 0x03, 0x01, 0x00, 0, 0, 0, 0, 0, 0, 0x07, 0x00,
        ],
    ))
    .expect("random-address parameters decode");
    state.configure(configuration(parameters));
    assert_eq!(
        state.enable_request(public_address, None),
        Err(LeLegacyAdvertisingRandomAddressMissing)
    );
    let random = BdAddr::new([9, 8, 7, 6, 5, 0xc4]);
    let Ok(LeLegacyAdvertisingEnableRequest::Nonconnectable(request)) =
        state.enable_request(public_address, Some(random))
    else {
        panic!("the random address completes the snapshot");
    };
    assert_eq!(
        request.advertiser(),
        LeLegacyAdvertisingAddress::Random(random)
    );
}
