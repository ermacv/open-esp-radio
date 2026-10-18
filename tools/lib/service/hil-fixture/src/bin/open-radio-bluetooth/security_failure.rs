#[cfg(target_os = "linux")]
mod peer {
    use super::super::{
        Result, connection_reset,
        hci::Socket,
        model::{
            PeerAddress,
            security_failure::{Observation, Report},
        },
    };
    use bt_hci::{
        FromHciBytes,
        cmd::{
            Cmd,
            le::LeEnableEncryption,
            link_control::{Disconnect, ReadRemoteVersionInformation},
        },
        data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
        event::Event,
        param::{ConnHandle, EncryptionEnabledLevel, Status},
    };
    use oer_hil_protocol::bluetooth::BluetoothSecurityFailure as Failure;
    use oer_hil_protocol::{
        bluetooth::BLUETOOTH_REFRESH_EDIV, bluetooth::BLUETOOTH_REFRESH_LTK,
        bluetooth::BLUETOOTH_REFRESH_RAND, bluetooth::BLUETOOTH_TEST_EDIV,
        bluetooth::BLUETOOTH_TEST_LTK, bluetooth::BLUETOOTH_TEST_RAND,
    };
    use std::time::{Duration, Instant};

    #[derive(Debug, PartialEq, Eq)]
    enum Action {
        Wait,
        ReadVersion,
        Refresh,
        SendData,
        Disconnect,
        Complete,
    }

    pub(crate) fn run(user: &Socket, peer: PeerAddress, report: &mut Report) -> Result<()> {
        let (handle, _, _) = connection_reset::connect(user, peer)?;
        report.connected = true;
        let started = Instant::now();
        user.send_command(&LeEnableEncryption::new(
            handle,
            BLUETOOTH_TEST_RAND,
            BLUETOOTH_TEST_EDIV,
            BLUETOOTH_TEST_LTK,
        ))?;
        let deadline = started + Duration::from_secs(8);
        loop {
            let packet = user.receive(deadline)?;
            if report.events.len() >= 64 || packet.len() > 258 {
                return Err("security failure HCI observation limit exceeded".into());
            }
            report.events.push(Observation {
                after_micros: started.elapsed().as_micros().try_into()?,
                packet: packet.clone(),
            });
            match observe(&packet, handle, report)? {
                Action::Wait => {}
                Action::Refresh => {
                    user.send_command(&LeEnableEncryption::new(
                        handle,
                        BLUETOOTH_REFRESH_RAND,
                        BLUETOOTH_REFRESH_EDIV,
                        BLUETOOTH_REFRESH_LTK,
                    ))?;
                }
                Action::SendData => {
                    let payload = oer_hil_protocol::bluetooth::bluetooth_peripheral_acl_payload();
                    user.send_acl(&AclPacket::new(
                        handle,
                        AclPacketBoundary::FirstNonFlushable,
                        AclBroadcastFlag::PointToPoint,
                        &payload,
                    ))?;
                    report.acl_sent = true;
                }
                Action::ReadVersion => {
                    user.send_command(&ReadRemoteVersionInformation::new(handle))?;
                }
                Action::Disconnect => {
                    // Initial key rejection preserves the ACL. Close explicitly after proving rejection.
                    user.send_command(&Disconnect::new(
                        handle,
                        bt_hci::param::DisconnectReason::RemoteUserTerminatedConn,
                    ))?;
                    report.disconnect_requested = true;
                }
                Action::Complete => {
                    report.completed_after_micros = Some(started.elapsed().as_micros().try_into()?);
                    return Ok(());
                }
            }
        }
    }

    fn observe(packet: &[u8], handle: ConnHandle, report: &mut Report) -> Result<Action> {
        if packet.first() != Some(&4) {
            return Err("application data during rejected encryption".into());
        }
        let (event, rest) = Event::from_hci_bytes(&packet[1..])
            .map_err(|e| format!("security failure event: {e:?}"))?;
        if !rest.is_empty() {
            return Err("trailing security failure event bytes".into());
        }
        match event {
            Event::CommandStatus(s) if s.cmd_opcode == LeEnableEncryption::OPCODE => {
                let admitted =
                    if report.failure == Failure::MissingRefreshKey && !report.initial_encrypted {
                        &mut report.initial_command_status
                    } else {
                        &mut report.command_status
                    };
                if s.status != Status::SUCCESS || *admitted {
                    return Err("failed or duplicate encryption command admission".into());
                }
                *admitted = true;
            }
            Event::CommandStatus(s) if s.cmd_opcode == Disconnect::OPCODE => {
                if !report.disconnect_requested
                    || s.status != Status::SUCCESS
                    || report.disconnect_command_status
                {
                    return Err("invalid cleanup Disconnect status".into());
                }
                report.disconnect_command_status = true;
            }
            Event::CommandStatus(s) if s.cmd_opcode == ReadRemoteVersionInformation::OPCODE => {
                if !report.read_version_before_disconnect
                    || report.encryption_failure != Some(6)
                    || report.version_command_status
                    || s.status != Status::SUCCESS
                {
                    return Err("invalid post-rejection remote-version command status".into());
                }
                report.version_command_status = true;
            }
            Event::ReadRemoteVersionInformationComplete(e)
                if report.read_version_before_disconnect =>
            {
                if !report.version_command_status
                    || report.remote_version.is_some()
                    || e.handle != handle
                    || e.status != Status::SUCCESS
                {
                    return Err("invalid post-rejection remote-version completion".into());
                }
                report.remote_version = Some((e.version.into_inner(), e.company_id, e.subversion));
                return Ok(Action::Disconnect);
            }
            Event::EncryptionChangeV1(e) => {
                if report.failure == Failure::ActiveDataMic {
                    if !report.command_status
                        || report.initial_encrypted
                        || report.acl_sent
                        || e.handle != handle
                        || e.status != Status::SUCCESS
                        || e.enabled != EncryptionEnabledLevel::OnE0OrAesCcm
                    {
                        return Err(
                            "active MIC injection requires successful initial encryption".into(),
                        );
                    }
                    report.initial_encrypted = true;
                    return Ok(Action::SendData);
                }
                if report.failure == Failure::MissingRefreshKey {
                    if !report.initial_command_status
                        || report.initial_encrypted
                        || report.command_status
                        || e.handle != handle
                        || e.status != Status::SUCCESS
                        || e.enabled != EncryptionEnabledLevel::OnE0OrAesCcm
                    {
                        return Err(
                            "refresh failure requires exactly one successful initial encryption"
                                .into(),
                        );
                    }
                    report.initial_encrypted = true;
                    return Ok(Action::Refresh);
                }
                let expected = match report.failure {
                    Failure::MissingKey | Failure::MissingRefreshKey => 6,
                    Failure::WrongKey | Failure::ActiveDataMic => 8,
                };
                if !report.command_status
                    || e.handle != handle
                    || e.status != Status::new(expected)
                    || e.enabled != EncryptionEnabledLevel::Off
                    || report.encryption_failure.is_some()
                {
                    return Err("unexpected encryption result for injected key failure".into());
                }
                report.encryption_failure = Some(expected);
                if report.failure == Failure::MissingKey {
                    return Ok(if report.read_version_before_disconnect {
                        Action::ReadVersion
                    } else {
                        Action::Disconnect
                    });
                }
            }
            Event::EncryptionKeyRefreshComplete(e) => {
                if report.failure != Failure::MissingRefreshKey
                    || !report.initial_encrypted
                    || !report.command_status
                    || e.handle != handle
                    || e.status != Status::new(6)
                    || report.encryption_failure.is_some()
                {
                    return Err("unexpected refresh result for injected key failure".into());
                }
                report.encryption_failure = Some(6);
            }
            Event::DisconnectionComplete(e) => {
                let expected = match report.failure {
                    Failure::MissingKey => 0x16,
                    Failure::WrongKey | Failure::ActiveDataMic => 8,
                    Failure::MissingRefreshKey => 6,
                };
                if !report.command_status
                    || e.handle != handle
                    || e.status != Status::SUCCESS
                    || e.reason != Status::new(expected)
                    || (report.failure == Failure::MissingKey
                        && (!report.disconnect_command_status
                            || report.encryption_failure != Some(6)))
                    || (report.failure == Failure::ActiveDataMic
                        && (!report.initial_encrypted || !report.acl_sent))
                {
                    return Err("unexpected peer disconnect for key failure".into());
                }
                report.disconnect_reason = Some(expected);
                return Ok(Action::Complete);
            }
            Event::HardwareError(_) => {
                return Err("adapter hardware error during key failure".into());
            }
            _ => {}
        }
        Ok(Action::Wait)
    }

    #[cfg(test)]
    mod tests {
        use super::super::super::model::Adapter;
        use super::*;
        fn report(failure: Failure) -> Report {
            Report::new(Adapter(0), PeerAddress([1; 6]), failure)
        }
        const STATUS: [u8; 7] = [4, 15, 4, 0, 1, 0x19, 0x20];
        const VERSION_STATUS: [u8; 7] = [4, 15, 4, 0, 1, 0x1d, 0x04];
        const VERSION: [u8; 11] = [4, 12, 8, 0, 1, 0, 0x0d, 0xff, 0xff, 1, 0];

        #[test]
        fn active_mic_requires_encryption_then_data_and_no_application_delivery() {
            let h = ConnHandle::new(1);
            let mut r = report(Failure::ActiveDataMic);
            let disconnect = [4, 5, 4, 0, 1, 0, 8];
            assert!(observe(&disconnect, h, &mut r).is_err());
            observe(&STATUS, h, &mut r).unwrap();
            assert!(observe(&disconnect, h, &mut r).is_err());
            assert_eq!(
                observe(&[4, 8, 4, 0, 1, 0, 1], h, &mut r).unwrap(),
                Action::SendData
            );
            assert!(observe(&disconnect, h, &mut r).is_err());
            r.acl_sent = true;
            assert!(observe(&[2, 1, 0, 0, 0], h, &mut r).is_err());
            assert!(observe(&[4, 8, 4, 0, 1, 0, 1], h, &mut r).is_err());
            assert_eq!(observe(&disconnect, h, &mut r).unwrap(), Action::Complete);
            assert!(!r.disconnect_requested);
        }

        fn rejected_with_version_probe() -> Report {
            let mut r = report(Failure::MissingKey);
            r.read_version_before_disconnect = true;
            let h = ConnHandle::new(1);
            observe(&STATUS, h, &mut r).unwrap();
            assert_eq!(
                observe(&[4, 8, 4, 6, 1, 0, 0], h, &mut r).unwrap(),
                Action::ReadVersion
            );
            r
        }

        #[test]
        fn version_diagnostic_requires_matching_completion_before_disconnect() {
            let h = ConnHandle::new(1);
            let mut r = rejected_with_version_probe();
            assert!(observe(&VERSION, h, &mut r).is_err());
            assert_eq!(observe(&VERSION_STATUS, h, &mut r).unwrap(), Action::Wait);
            assert_eq!(observe(&VERSION, h, &mut r).unwrap(), Action::Disconnect);
            assert_eq!(r.remote_version, Some((0x0d, 0xffff, 1)));
            assert!(observe(&VERSION, h, &mut r).is_err());
            assert!(observe(&VERSION_STATUS, h, &mut r).is_err());
            assert!(!r.disconnect_requested);
            r.disconnect_requested = true;
            observe(&[4, 15, 4, 0, 1, 6, 4], h, &mut r).unwrap();
            assert_eq!(
                observe(&[4, 5, 4, 0, 1, 0, 0x16], h, &mut r).unwrap(),
                Action::Complete
            );
        }

        #[test]
        fn version_diagnostic_rejects_failed_command_foreign_handle_and_early_disconnect() {
            let h = ConnHandle::new(1);
            let mut early = report(Failure::MissingKey);
            early.read_version_before_disconnect = true;
            assert!(observe(&VERSION_STATUS, h, &mut early).is_err());
            let mut rejected = rejected_with_version_probe();
            assert!(observe(&[4, 15, 4, 0x0c, 1, 0x1d, 4], h, &mut rejected).is_err());
            for (index, value) in [(3, 8), (4, 2)] {
                let mut r = rejected_with_version_probe();
                observe(&VERSION_STATUS, h, &mut r).unwrap();
                let mut packet = VERSION;
                packet[index] = value;
                assert!(observe(&packet, h, &mut r).is_err());
                assert!(r.remote_version.is_none());
            }
            // A foreign identity is recorded for the runner to judge.
            for (index, value) in [(6, 0x0c), (7, 0), (9, 2)] {
                let mut r = rejected_with_version_probe();
                observe(&VERSION_STATUS, h, &mut r).unwrap();
                let mut packet = VERSION;
                packet[index] = value;
                observe(&packet, h, &mut r).unwrap();
                assert!(
                    r.remote_version
                        .is_some_and(|identity| identity != (0x0d, 0xffff, 1))
                );
            }
            let mut r = rejected_with_version_probe();
            assert!(observe(&[4, 5, 4, 0, 1, 0, 0x16], h, &mut r).is_err());
        }
        fn awaiting_refresh_failure() -> Report {
            let h = ConnHandle::new(1);
            let mut r = report(Failure::MissingRefreshKey);
            observe(&STATUS, h, &mut r).unwrap();
            assert!(!r.command_status);
            assert!(observe(&STATUS, h, &mut r).is_err());
            assert_eq!(
                observe(&[4, 8, 4, 0, 1, 0, 1], h, &mut r).unwrap(),
                Action::Refresh
            );
            observe(&STATUS, h, &mut r).unwrap();
            r
        }

        #[test]
        fn missing_refresh_key_requires_initial_encryption_and_remote_termination() {
            let h = ConnHandle::new(1);
            let mut early = report(Failure::MissingRefreshKey);
            assert!(observe(&[4, 8, 4, 0, 1, 0, 1], h, &mut early).is_err());
            observe(&STATUS, h, &mut early).unwrap();
            assert!(observe(&[4, 5, 4, 0, 1, 0, 6], h, &mut early).is_err());
            for notification in [false, true] {
                let mut r = awaiting_refresh_failure();
                if notification {
                    assert_eq!(
                        observe(&[4, 0x30, 3, 6, 1, 0], h, &mut r).unwrap(),
                        Action::Wait
                    );
                    assert!(observe(&[4, 0x30, 3, 6, 1, 0], h, &mut r).is_err());
                }
                assert_eq!(
                    observe(&[4, 5, 4, 0, 1, 0, 6], h, &mut r).unwrap(),
                    Action::Complete
                );
                assert!(!r.disconnect_requested);
            }
            for packet in [
                vec![4, 0x30, 3, 0, 1, 0],    // refresh must not succeed
                vec![4, 0x30, 3, 6, 2, 0],    // foreign handle
                vec![4, 8, 4, 0, 1, 0, 1],    // duplicate initial success
                vec![4, 5, 4, 0, 1, 0, 8],    // timeout cannot replace termination
                vec![4, 5, 4, 0, 1, 0, 0x16], // no local cleanup command
                vec![2, 1, 0, 0, 0],          // no application delivery
            ] {
                assert!(observe(&packet, h, &mut awaiting_refresh_failure()).is_err());
            }
        }

        #[test]
        fn missing_key_requires_rejection_before_explicit_disconnect() {
            let h = ConnHandle::new(1);
            let mut r = report(Failure::MissingKey);
            let rejected = [4, 8, 4, 6, 1, 0, 0];
            assert!(observe(&rejected, h, &mut r).is_err());
            observe(&STATUS, h, &mut r).unwrap();
            assert_eq!(observe(&rejected, h, &mut r).unwrap(), Action::Disconnect);
            assert!(observe(&[4, 5, 4, 0, 1, 0, 0x16], h, &mut r).is_err());
            r.disconnect_requested = true;
            observe(&[4, 15, 4, 0, 1, 6, 4], h, &mut r).unwrap();
            assert_eq!(
                observe(&[4, 5, 4, 0, 1, 0, 0x16], h, &mut r).unwrap(),
                Action::Complete
            );
        }
        #[test]
        fn wrong_key_never_accepts_success_data_foreign_handle_or_wrong_reason() {
            let h = ConnHandle::new(1);
            for packet in [
                vec![4, 8, 4, 0, 1, 0, 1],
                vec![2, 1, 0, 0, 0],
                vec![4, 5, 4, 0, 2, 0, 8],
                vec![4, 5, 4, 0, 1, 0, 0x13],
                vec![4, 5, 4, 0, 1, 0, 8, 0],
            ] {
                let mut r = report(Failure::WrongKey);
                observe(&STATUS, h, &mut r).unwrap();
                assert!(observe(&packet, h, &mut r).is_err());
            }
            let mut r = report(Failure::WrongKey);
            observe(&STATUS, h, &mut r).unwrap();
            assert_eq!(
                observe(&[4, 5, 4, 0, 1, 0, 8], h, &mut r).unwrap(),
                Action::Complete
            );
            assert!(observe(&STATUS, h, &mut r).is_err());
        }
    }
}
#[cfg(target_os = "linux")]
pub(super) use peer::run;
