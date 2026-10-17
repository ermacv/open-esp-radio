use bt_hci::{
    FromHciBytes,
    event::{CommandStatus, Event, le::LeEvent},
    param::{
        AddrKind, BdAddr, ClockAccuracy, ConnHandle, Duration, Error as HciError, LeConnRole,
        Status,
    },
};

use super::{
    LeConnectionUpdateCompleteEvent, LeDisconnectCommand, LeDisconnectCommandStatusEvent,
    LeDisconnectDecodeError, LeDisconnectionCompleteEvent, LeEncryptionChangeEvent,
    LeEncryptionKeyRefreshCompleteEvent, LeLongTermKeyCommandDecodeError,
    LeLongTermKeyRequestEvent, LeLongTermKeyRequestNegativeReplyCommand,
    LeLongTermKeyRequestReplyCommand, LePeripheralConnectionCompleteEvent,
    LePeripheralConnectionCompleteEventError, LeReadRemoteFeaturesCommand,
    LeReadRemoteFeaturesCommandStatusEvent, LeReadRemoteFeaturesCompleteEvent,
    LeReadRemoteFeaturesDecodeError, LeReadRemoteVersionInformationCommand,
    LeReadRemoteVersionInformationCommandStatusEvent, LeReadRemoteVersionInformationCompleteEvent,
    LeReadRemoteVersionInformationDecodeError,
};
use crate::HciCommandPacket;
use std::format;

#[test]
fn disconnect_decode_owns_validated_handle_and_reason() {
    let command = LeDisconnectCommand::decode(HciCommandPacket::new(
        LeDisconnectCommand::OPCODE,
        &[0xbc, 0x0a, 0x13],
    ))
    .expect("the standard parameter body is valid");
    assert_eq!(command.handle(), ConnHandle::new(0x0abc));
    assert_eq!(command.reason(), 0x13);

    for parameters in [
        &[][..],
        &[1, 0][..],
        &[1, 0, 0x16][..],
        &[0, 0x10, 0x13][..],
    ] {
        assert_eq!(
            LeDisconnectCommand::decode(HciCommandPacket::new(
                LeDisconnectCommand::OPCODE,
                parameters,
            )),
            Err(LeDisconnectDecodeError::Malformed)
        );
    }
}

#[test]
fn disconnect_status_roundtrips_through_bt_hci() {
    let event = LeDisconnectCommandStatusEvent::new(Status::SUCCESS);
    let Event::CommandStatus(decoded) = Event::from_hci_bytes_complete(event.as_bytes())
        .expect("bt-hci decodes the complete Command Status")
    else {
        panic!("Disconnect response changed standard event kind");
    };
    let _: CommandStatus = decoded;
    assert_eq!(decoded.status, Status::SUCCESS);
    assert_eq!(decoded.num_hci_cmd_pkts, 1);
    assert_eq!(decoded.cmd_opcode, LeDisconnectCommand::OPCODE);
}

#[test]
fn read_remote_features_command_and_events_use_standard_wire_shapes() {
    let command = LeReadRemoteFeaturesCommand::decode(HciCommandPacket::new(
        LeReadRemoteFeaturesCommand::OPCODE,
        &[0xbc, 0x0a],
    ))
    .unwrap();
    assert_eq!(command.handle(), ConnHandle::new(0x0abc));
    assert_eq!(
        LeReadRemoteFeaturesCommand::decode(HciCommandPacket::new(
            LeReadRemoteFeaturesCommand::OPCODE,
            &[0, 0x10],
        )),
        Err(LeReadRemoteFeaturesDecodeError::Malformed)
    );

    let status = LeReadRemoteFeaturesCommandStatusEvent::new(Status::SUCCESS);
    let Event::CommandStatus(decoded) = Event::from_hci_bytes_complete(status.as_bytes()).unwrap()
    else {
        panic!("remote-feature status changed event kind");
    };
    assert_eq!(decoded.cmd_opcode, LeReadRemoteFeaturesCommand::OPCODE);
    assert_eq!(decoded.status, Status::SUCCESS);

    let features = [1, 2, 3, 4, 5, 6, 7, 8];
    let complete =
        LeReadRemoteFeaturesCompleteEvent::new(Status::SUCCESS, ConnHandle::new(0x0abc), features);
    let Event::Le(LeEvent::LeReadRemoteFeaturesComplete(decoded)) =
        Event::from_hci_bytes_complete(complete.as_bytes()).unwrap()
    else {
        panic!("remote-feature completion changed event kind");
    };
    assert_eq!(decoded.status, Status::SUCCESS);
    assert_eq!(decoded.handle, ConnHandle::new(0x0abc));
    assert!(decoded.le_features.supports_le_encryption());
    assert_eq!(&complete.as_bytes()[6..], &features);
}

#[test]
fn read_remote_version_command_and_events_use_standard_wire_shapes() {
    let command = LeReadRemoteVersionInformationCommand::decode(HciCommandPacket::new(
        LeReadRemoteVersionInformationCommand::OPCODE,
        &[0xbc, 0x0a],
    ))
    .unwrap();
    assert_eq!(command.handle(), ConnHandle::new(0x0abc));
    assert_eq!(
        LeReadRemoteVersionInformationCommand::decode(HciCommandPacket::new(
            LeReadRemoteVersionInformationCommand::OPCODE,
            &[0, 0x10],
        )),
        Err(LeReadRemoteVersionInformationDecodeError::Malformed)
    );

    let status = LeReadRemoteVersionInformationCommandStatusEvent::new(Status::SUCCESS);
    let Event::CommandStatus(decoded) = Event::from_hci_bytes_complete(status.as_bytes()).unwrap()
    else {
        panic!("remote-version status changed event kind");
    };
    assert_eq!(
        decoded.cmd_opcode,
        LeReadRemoteVersionInformationCommand::OPCODE
    );
    assert_eq!(decoded.status, Status::SUCCESS);

    let complete = LeReadRemoteVersionInformationCompleteEvent::new(
        Status::SUCCESS,
        ConnHandle::new(0x0abc),
        0x0d,
        0x1234,
        0x5678,
    );
    let Event::ReadRemoteVersionInformationComplete(decoded) =
        Event::from_hci_bytes_complete(complete.as_bytes()).unwrap()
    else {
        panic!("remote-version completion changed event kind");
    };
    assert_eq!(decoded.status, Status::SUCCESS);
    assert_eq!(decoded.handle, ConnHandle::new(0x0abc));
    assert_eq!(decoded.company_id, 0x1234);
    assert_eq!(decoded.subversion, 0x5678);
    assert_eq!(&complete.as_bytes()[5..], &[0x0d, 0x34, 0x12, 0x78, 0x56]);
}

#[test]
fn long_term_key_commands_transfer_a_redacted_secret_and_use_standard_completions() {
    let key = [0x5a; 16];
    let mut parameters = [0; 18];
    parameters[..2].copy_from_slice(&0x0abcu16.to_le_bytes());
    parameters[2..].copy_from_slice(&key);
    let reply = LeLongTermKeyRequestReplyCommand::decode(HciCommandPacket::new(
        LeLongTermKeyRequestReplyCommand::OPCODE,
        &parameters,
    ))
    .unwrap();
    assert_eq!(reply.handle(), ConnHandle::new(0x0abc));
    assert!(!format!("{reply:?}").contains("5a"));
    assert_eq!(reply.into_long_term_key(), key);

    let negative = LeLongTermKeyRequestNegativeReplyCommand::decode(HciCommandPacket::new(
        LeLongTermKeyRequestNegativeReplyCommand::OPCODE,
        &[0xbc, 0x0a],
    ))
    .unwrap();
    let complete = negative.into_accepted_complete();
    let Event::CommandComplete(decoded) =
        Event::from_hci_bytes_complete(complete.as_bytes()).unwrap()
    else {
        panic!("LTK response changed event kind");
    };
    assert_eq!(decoded.num_hci_cmd_pkts, 1);
    assert_eq!(
        decoded.cmd_opcode,
        LeLongTermKeyRequestNegativeReplyCommand::OPCODE
    );
    assert_eq!(&*decoded.bytes, &[0, 0xbc, 0x0a]);

    for parameters in [&[][..], &[1, 0][..], &[0; 17][..], &[0; 19][..]] {
        assert!(matches!(
            LeLongTermKeyRequestReplyCommand::decode(HciCommandPacket::new(
                LeLongTermKeyRequestReplyCommand::OPCODE,
                parameters,
            )),
            Err(LeLongTermKeyCommandDecodeError::Malformed)
        ));
    }
    assert_eq!(
        LeLongTermKeyRequestNegativeReplyCommand::decode(HciCommandPacket::new(
            LeLongTermKeyRequestNegativeReplyCommand::OPCODE,
            &[0, 0x10],
        )),
        Err(LeLongTermKeyCommandDecodeError::Malformed)
    );
}

#[test]
fn encryption_events_roundtrip_through_bt_hci() {
    let request =
        LeLongTermKeyRequestEvent::new(ConnHandle::new(0x0abc), [1, 2, 3, 4, 5, 6, 7, 8], 0x1234);
    let Event::Le(LeEvent::LeLongTermKeyRequest(decoded)) =
        Event::from_hci_bytes_complete(request.as_bytes()).unwrap()
    else {
        panic!("LTK request changed event kind");
    };
    assert_eq!(decoded.handle, ConnHandle::new(0x0abc));
    assert_eq!(decoded.random_number, [1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(decoded.encrypted_diversifier, 0x1234);

    let changed = LeEncryptionChangeEvent::new(Status::SUCCESS, ConnHandle::new(0x0abc), true);
    let Event::EncryptionChangeV1(decoded) =
        Event::from_hci_bytes_complete(changed.as_bytes()).unwrap()
    else {
        panic!("encryption change changed event kind");
    };
    assert_eq!(decoded.status, Status::SUCCESS);
    assert_eq!(decoded.handle, ConnHandle::new(0x0abc));
    assert_eq!(
        decoded.enabled,
        bt_hci::param::EncryptionEnabledLevel::OnE0OrAesCcm
    );

    let refreshed =
        LeEncryptionKeyRefreshCompleteEvent::new(Status::SUCCESS, ConnHandle::new(0x0abc));
    let Event::EncryptionKeyRefreshComplete(decoded) =
        Event::from_hci_bytes_complete(refreshed.as_bytes()).unwrap()
    else {
        panic!("key refresh changed event kind");
    };
    assert_eq!(decoded.status, Status::SUCCESS);
    assert_eq!(decoded.handle, ConnHandle::new(0x0abc));
}

#[test]
fn peripheral_connection_complete_roundtrips_through_bt_hci() {
    let event = LePeripheralConnectionCompleteEvent::new(
        ConnHandle::new(0x0abc),
        AddrKind::RANDOM,
        BdAddr::new([1, 2, 3, 4, 5, 0xc6]),
        Duration::from_u16(24),
        3,
        Duration::from_u16(200),
        ClockAccuracy::Ppm75,
    )
    .expect("the legacy peer address is representable");

    let Event::Le(LeEvent::LeConnectionComplete(decoded)) =
        Event::from_hci_bytes_complete(event.as_bytes())
            .expect("bt-hci decodes the complete standard event")
    else {
        panic!("the emitted event changed standard kind");
    };
    assert_eq!(decoded.status, Status::SUCCESS);
    assert_eq!(decoded.handle, ConnHandle::new(0x0abc));
    assert_eq!(decoded.role, LeConnRole::Peripheral);
    assert_eq!(decoded.peer_addr_kind, AddrKind::RANDOM);
    assert_eq!(decoded.peer_addr, BdAddr::new([1, 2, 3, 4, 5, 0xc6]));
    assert_eq!(decoded.conn_interval, Duration::from_u16(24));
    assert_eq!(decoded.peripheral_latency, 3);
    assert_eq!(decoded.supervision_timeout, Duration::from_u16(200));
    assert_eq!(decoded.central_clock_accuracy, ClockAccuracy::Ppm75);
}

#[test]
fn peripheral_connection_complete_rejects_non_legacy_peer_address_kind() {
    assert_eq!(
        LePeripheralConnectionCompleteEvent::new(
            ConnHandle::new(1),
            AddrKind::RESOLVABLE_PRIVATE_OR_PUBLIC,
            BdAddr::default(),
            Duration::from_u16(24),
            0,
            Duration::from_u16(200),
            ClockAccuracy::Ppm500,
        ),
        Err(
            LePeripheralConnectionCompleteEventError::UnsupportedPeerAddressKind(
                AddrKind::RESOLVABLE_PRIVATE_OR_PUBLIC,
            )
        )
    );
}

#[test]
fn failed_connection_complete_has_no_allocated_connection() {
    let status = HciError::CONN_FAILED_SYNCHRONIZATION_TIMEOUT.to_status();
    let event = LePeripheralConnectionCompleteEvent::failed(status)
        .expect("a non-success status represents failed establishment");

    let Event::Le(LeEvent::LeConnectionComplete(decoded)) =
        Event::from_hci_bytes_complete(event.as_bytes())
            .expect("bt-hci decodes the failed standard event")
    else {
        panic!("the emitted event changed standard kind");
    };
    assert_eq!(decoded.status, status);
    assert_eq!(decoded.handle, ConnHandle::new(0));
    assert_eq!(decoded.conn_interval, Duration::from_u16(0));
    assert_eq!(decoded.supervision_timeout, Duration::from_u16(0));

    assert_eq!(
        LePeripheralConnectionCompleteEvent::failed(Status::SUCCESS),
        Err(LePeripheralConnectionCompleteEventError::SuccessfulFailureStatus)
    );
}

#[test]
fn disconnection_complete_roundtrips_through_bt_hci() {
    let reason = HciError::CONN_TIMEOUT.to_status();
    let event = LeDisconnectionCompleteEvent::new(ConnHandle::new(0x0abc), reason);

    let Event::DisconnectionComplete(decoded) = Event::from_hci_bytes_complete(event.as_bytes())
        .expect("bt-hci decodes the complete standard event")
    else {
        panic!("the emitted event changed standard kind");
    };
    assert_eq!(decoded.status, Status::SUCCESS);
    assert_eq!(decoded.handle, ConnHandle::new(0x0abc));
    assert_eq!(decoded.reason, reason);
}

#[test]
fn connection_update_complete_roundtrips_through_bt_hci() {
    let event = LeConnectionUpdateCompleteEvent::new(
        ConnHandle::new(0x0abc),
        Duration::from_u16(40),
        3,
        Duration::from_u16(200),
    );

    let Event::Le(LeEvent::LeConnectionUpdateComplete(decoded)) =
        Event::from_hci_bytes_complete(event.as_bytes())
            .expect("bt-hci decodes the complete standard event")
    else {
        panic!("the emitted event changed standard kind");
    };
    assert_eq!(decoded.status, Status::SUCCESS);
    assert_eq!(decoded.handle, ConnHandle::new(0x0abc));
    assert_eq!(decoded.conn_interval, Duration::from_u16(40));
    assert_eq!(decoded.peripheral_latency, 3);
    assert_eq!(decoded.supervision_timeout, Duration::from_u16(200));
}
