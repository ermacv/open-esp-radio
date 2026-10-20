//! A fixed LE 1M connection and ACL echo followed by Reset, without Disconnect.

use super::{
    Result,
    hci::Socket,
    model::{ConnectionReset, PeerAddress},
};
use bt_hci::{
    FromHciBytes,
    cmd::{
        Cmd,
        controller_baseband::SetEventMask,
        le::{
            LeConnUpdate, LeCreateConn, LeEnableEncryption, LeReadChannelMap, LeReadRemoteFeatures,
            LeSetEventMask, LeSetHostChannelClassification,
        },
        link_control::ReadRemoteVersionInformation,
    },
    data::{AclBroadcastFlag, AclPacket, AclPacketBoundary},
    event::{Event, le::LeEvent},
    param::{
        AddrKind, BdAddr, ChannelMap, ConnHandle, Duration as HciDuration, EventMask, LeConnRole,
        LeEventMask,
    },
};
use oer_hil_protocol::{
    bluetooth::BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES,
    bluetooth::BLUETOOTH_PERIPHERAL_UPDATED_INTERVAL_MILLIS,
    bluetooth::BluetoothPeripheralTermination, bluetooth::bluetooth_peripheral_acl_payload,
    bluetooth::bluetooth_peripheral_acl_payload_for_sequence,
};
use std::time::{Duration, Instant};

pub(super) const ACL_ECHO_PAYLOAD: [u8; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES] =
    bluetooth_peripheral_acl_payload();
pub(super) const POST_UPDATE_ACL_ECHO_PAYLOAD: [u8; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES] =
    bluetooth_peripheral_acl_payload_for_sequence(1);

pub(super) enum ConnectionRunOutcome {
    Complete,
    PeerRfkill { connected: Instant },
}

struct AclEchoAssembly {
    bytes: [u8; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES],
    len: usize,
    hci_packets: u16,
}

impl AclEchoAssembly {
    const fn new() -> Self {
        Self {
            bytes: [0; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES],
            len: 0,
            hci_packets: 0,
        }
    }

    fn receive(
        &mut self,
        packet: &[u8],
        handle: ConnHandle,
        expected: &[u8; BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES],
    ) -> Result<bool> {
        match packet.first() {
            Some(&2) => {
                let (packet, rest) = AclPacket::from_hci_bytes(&packet[1..])
                    .map_err(|error| format!("invalid HCI ACL echo: {error:?}"))?;
                if !rest.is_empty() {
                    return Err("trailing HCI ACL echo bytes".into());
                }
                if packet.handle() != handle
                    || packet.broadcast_flag() != AclBroadcastFlag::PointToPoint
                {
                    return Err("HCI ACL echo does not match the active connection".into());
                }
                match packet.boundary_flag() {
                    AclPacketBoundary::FirstFlushable if self.len == 0 => {}
                    AclPacketBoundary::Continuing if self.len != 0 => {}
                    _ => return Err("HCI ACL echo has an invalid fragment boundary".into()),
                }
                let end = self
                    .len
                    .checked_add(packet.data().len())
                    .filter(|end| *end <= self.bytes.len())
                    .ok_or("HCI ACL echo exceeds the expected payload")?;
                if packet.data() != &expected[self.len..end] {
                    return Err(
                        "HCI ACL echo fragment does not match the transmitted payload".into(),
                    );
                }
                self.bytes[self.len..end].copy_from_slice(packet.data());
                self.len = end;
                self.hci_packets = self
                    .hci_packets
                    .checked_add(1)
                    .ok_or("HCI ACL echo fragment counter exhausted")?;
                Ok(self.len == self.bytes.len())
            }
            Some(&4) => {
                let (event, rest) = Event::from_hci_bytes(&packet[1..]).map_err(|error| {
                    format!("invalid HCI event while awaiting ACL echo: {error:?}")
                })?;
                if !rest.is_empty() {
                    return Err("trailing HCI event bytes while awaiting ACL echo".into());
                }
                match event {
                    Event::HardwareError(_) => {
                        Err("controller hardware error during ACL echo".into())
                    }
                    Event::DisconnectionComplete(_) => {
                        Err("connection ended before the ACL echo arrived".into())
                    }
                    _ => Ok(false),
                }
            }
            _ => Err("unexpected HCI packet while awaiting ACL echo".into()),
        }
    }
}

pub(super) fn run(
    user: &Socket,
    peer: PeerAddress,
    hold_ms: u16,
    termination: BluetoothPeripheralTermination,
    report: &mut ConnectionReset,
) -> Result<ConnectionRunOutcome> {
    let (handle, connected, elapsed) = connect(user, peer)?;
    report.connection_complete = true;
    report.connection_after_micros = Some(elapsed);

    read_remote_features(user, handle, report)?;
    read_remote_version(user, handle, report)?;

    if report.encrypted {
        enable_encryption(user, handle, report, false)?;
    }

    let acl = AclPacket::new(
        handle,
        AclPacketBoundary::FirstNonFlushable,
        AclBroadcastFlag::PointToPoint,
        &ACL_ECHO_PAYLOAD,
    );
    user.send_acl(&acl)?;
    report.acl_sent = true;
    report.acl_payload_bytes = Some(ACL_ECHO_PAYLOAD.len() as u16);
    let acl_started = Instant::now();
    let acl_deadline = acl_started + Duration::from_secs(5);
    let mut echo = AclEchoAssembly::new();
    loop {
        let packet = user.receive(acl_deadline)?;
        if echo.receive(&packet, handle, &ACL_ECHO_PAYLOAD)? {
            report.acl_echo_received = true;
            report.acl_echo_hci_packets = Some(echo.hci_packets);
            report.acl_echo_after_micros = Some(
                acl_started
                    .elapsed()
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
            break;
        }
    }

    let updated_interval =
        HciDuration::from_millis(u32::from(BLUETOOTH_PERIPHERAL_UPDATED_INTERVAL_MILLIS));
    let update = LeConnUpdate::new(
        handle,
        updated_interval,
        updated_interval,
        0,
        HciDuration::from_millis(2_000),
        HciDuration::from_millis(0),
        HciDuration::from_millis(0),
    );
    let update_started = Instant::now();
    user.send_command(&update)?;
    let update_deadline = update_started + Duration::from_secs(5);
    loop {
        let packet = user.receive(update_deadline)?;
        if connection_updated(&packet, handle, updated_interval)? {
            report.connection_update_complete = true;
            report.updated_interval_millis = Some(BLUETOOTH_PERIPHERAL_UPDATED_INTERVAL_MILLIS);
            report.connection_update_after_micros = Some(
                update_started
                    .elapsed()
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
            break;
        }
    }

    let mut channel_map = ChannelMap::new();
    for channel in 2..37 {
        channel_map.set_channel_bad(channel, true);
    }
    let channel_map_started = Instant::now();
    user.command(LeSetHostChannelClassification::new(channel_map))?;
    let channel_map_deadline = channel_map_started + Duration::from_secs(5);
    loop {
        let observed = user.command(LeReadChannelMap::new(handle))?;
        if channel_maps_match(&observed.channel_map, &channel_map) {
            report.channel_map_updated = true;
            report.channel_map_update_after_micros = Some(
                channel_map_started
                    .elapsed()
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
            break;
        }
        if Instant::now() >= channel_map_deadline {
            return Err("Channel Map Update was not applied before the deadline".into());
        }
        oer_os_process::sleep(Duration::from_millis(20))?;
    }

    if report.key_refresh {
        enable_encryption(user, handle, report, true)?;
    }

    let post_update_acl = AclPacket::new(
        handle,
        AclPacketBoundary::FirstNonFlushable,
        AclBroadcastFlag::PointToPoint,
        &POST_UPDATE_ACL_ECHO_PAYLOAD,
    );
    user.send_acl(&post_update_acl)?;
    report.post_update_acl_sent = true;
    let post_update_acl_started = Instant::now();
    let post_update_acl_deadline = post_update_acl_started + Duration::from_secs(5);
    let mut post_update_echo = AclEchoAssembly::new();
    loop {
        let packet = user.receive(post_update_acl_deadline)?;
        if post_update_echo.receive(&packet, handle, &POST_UPDATE_ACL_ECHO_PAYLOAD)? {
            report.post_update_acl_echo_received = true;
            report.post_update_acl_echo_hci_packets = Some(post_update_echo.hci_packets);
            report.post_update_acl_echo_after_micros = Some(
                post_update_acl_started
                    .elapsed()
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
            break;
        }
    }

    if termination == BluetoothPeripheralTermination::PeerRfkill {
        return Ok(ConnectionRunOutcome::PeerRfkill { connected });
    }
    finish_connection(user, handle, hold_ms, termination, connected, report)?;
    Ok(ConnectionRunOutcome::Complete)
}

pub(super) fn connect(user: &Socket, peer: PeerAddress) -> Result<(ConnHandle, Instant, u64)> {
    user.command(SetEventMask::new(
        EventMask::new()
            .enable_le_meta(true)
            .enable_hardware_error(true)
            .enable_encryption_change_v1(true)
            .enable_encryption_key_refresh_complete(true)
            .enable_disconnection_complete(true)
            .enable_read_remote_version_information_complete(true),
    ))?;
    // Request the legacy completion only, so there is one exact peer/role check.
    user.command(LeSetEventMask::new(
        LeEventMask::new()
            .enable_le_conn_complete(true)
            .enable_le_conn_update_complete(true)
            .enable_le_read_remote_features_page_0_complete(true),
    ))?;
    let command = LeCreateConn::new(
        HciDuration::from_millis(60),
        HciDuration::from_millis(30),
        false,
        AddrKind::PUBLIC,
        BdAddr::new(peer.0),
        AddrKind::PUBLIC,
        HciDuration::from_millis(100),
        HciDuration::from_millis(100),
        0,
        HciDuration::from_millis(2_000),
        HciDuration::from_millis(0),
        HciDuration::from_millis(0),
    );
    let started = Instant::now();
    user.send_command(&command)?;
    let deadline = started + Duration::from_secs(10);
    let handle = loop {
        let packet = user.receive(deadline)?;
        if let Some(handle) = connection_created(&packet, peer)? {
            break handle;
        }
    };
    let connected = Instant::now();
    Ok((
        handle,
        connected,
        connected.duration_since(started).as_micros() as u64,
    ))
}

fn read_remote_features(
    user: &Socket,
    handle: ConnHandle,
    report: &mut ConnectionReset,
) -> Result<()> {
    let started = Instant::now();
    user.send_command(&LeReadRemoteFeatures::new(handle))?;
    let deadline = started + Duration::from_secs(5);
    let mut accepted = false;
    loop {
        let packet = user.receive(deadline)?;
        let event = decode_event(&packet, "remote feature exchange")?;
        match event {
            Event::CommandStatus(status) if status.cmd_opcode == LeReadRemoteFeatures::OPCODE => {
                status
                    .status
                    .to_result()
                    .map_err(|error| format!("LE Read Remote Features rejected: {error:?}"))?;
                if accepted {
                    return Err("duplicate LE Read Remote Features Command Status".into());
                }
                accepted = true;
                report.remote_features_command_status = true;
            }
            Event::Le(LeEvent::LeReadRemoteFeaturesComplete(complete)) => {
                if !accepted {
                    return Err(
                        "LE Read Remote Features completed before its successful Command Status"
                            .into(),
                    );
                }
                complete
                    .status
                    .to_result()
                    .map_err(|error| format!("remote feature exchange failed: {error:?}"))?;
                let features = complete.le_features.into_inner();
                // The runner judges the features against the target profile.
                if complete.handle != handle {
                    return Err("remote feature completion names another connection".into());
                }
                report.remote_features_complete = true;
                report.remote_features = Some(features);
                report.remote_features_after_micros =
                    Some(started.elapsed().as_micros().try_into().unwrap_or(u64::MAX));
                return Ok(());
            }
            Event::HardwareError(_) => {
                return Err("controller hardware error during remote feature exchange".into());
            }
            Event::DisconnectionComplete(_) => {
                return Err("connection ended during remote feature exchange".into());
            }
            _ => {}
        }
    }
}

fn read_remote_version(
    user: &Socket,
    handle: ConnHandle,
    report: &mut ConnectionReset,
) -> Result<()> {
    let started = Instant::now();
    user.send_command(&ReadRemoteVersionInformation::new(handle))?;
    let deadline = started + Duration::from_secs(5);
    let mut accepted = false;
    loop {
        let packet = user.receive(deadline)?;
        let event = decode_event(&packet, "remote version exchange")?;
        match event {
            Event::CommandStatus(status)
                if status.cmd_opcode == ReadRemoteVersionInformation::OPCODE =>
            {
                status.status.to_result().map_err(|error| {
                    format!("Read Remote Version Information rejected: {error:?}")
                })?;
                if accepted {
                    return Err("duplicate Read Remote Version Information Command Status".into());
                }
                accepted = true;
                report.remote_version_command_status = true;
            }
            Event::ReadRemoteVersionInformationComplete(complete) => {
                if !accepted {
                    return Err(
                        "Read Remote Version Information completed before its successful Command Status"
                            .into(),
                    );
                }
                complete
                    .status
                    .to_result()
                    .map_err(|error| format!("remote version exchange failed: {error:?}"))?;
                let version = complete.version.into_inner();
                // The runner judges the identity against the target profile.
                if complete.handle != handle {
                    return Err("remote version completion names another connection".into());
                }
                report.remote_version_complete = true;
                report.remote_version = Some(version);
                report.remote_version_company = Some(complete.company_id);
                report.remote_version_subversion = Some(complete.subversion);
                report.remote_version_after_micros =
                    Some(started.elapsed().as_micros().try_into().unwrap_or(u64::MAX));
                return Ok(());
            }
            Event::HardwareError(_) => {
                return Err("controller hardware error during remote version exchange".into());
            }
            Event::DisconnectionComplete(_) => {
                return Err("connection ended during remote version exchange".into());
            }
            _ => {}
        }
    }
}

fn decode_event<'packet>(packet: &'packet [u8], context: &str) -> Result<Event<'packet>> {
    if packet.first() != Some(&4) {
        return Err(format!("unexpected non-event HCI packet during {context}").into());
    }
    let (event, rest) = Event::from_hci_bytes(&packet[1..])
        .map_err(|error| format!("invalid HCI event during {context}: {error:?}"))?;
    if !rest.is_empty() {
        return Err(format!("trailing HCI event bytes during {context}").into());
    }
    Ok(event)
}

fn channel_maps_match(observed: &ChannelMap, expected: &ChannelMap) -> bool {
    (0..37).all(|channel| observed.is_channel_bad(channel) == expected.is_channel_bad(channel))
}

fn finish_connection(
    user: &Socket,
    handle: ConnHandle,
    hold_ms: u16,
    termination: BluetoothPeripheralTermination,
    connected: Instant,
    report: &mut ConnectionReset,
) -> Result<()> {
    match termination {
        BluetoothPeripheralTermination::PeerReset => {
            oer_os_process::sleep(Duration::from_millis(u64::from(hold_ms)))?;
            // Capture when Reset is sent; its completion can arrive substantially later.
            report.termination_after_connection_micros =
                Some(connected.elapsed().as_micros() as u64);
            user.command(bt_hci::cmd::controller_baseband::Reset::new())?;
            report.reset_completed = true;
        }
        BluetoothPeripheralTermination::TargetDisconnect
        | BluetoothPeripheralTermination::TargetReset => {
            let expected_reason = match termination {
                BluetoothPeripheralTermination::TargetDisconnect => 0x13,
                BluetoothPeripheralTermination::TargetReset => 0x08,
                BluetoothPeripheralTermination::PeerReset
                | BluetoothPeripheralTermination::PeerRfkill => unreachable!(),
            };
            let deadline =
                Instant::now() + Duration::from_millis(u64::from(hold_ms)) + Duration::from_secs(8);
            loop {
                let packet = user.receive(deadline)?;
                if peer_disconnected(&packet, handle, expected_reason)? {
                    report.peer_disconnection_complete = true;
                    report.peer_disconnect_reason = Some(expected_reason);
                    report.termination_after_connection_micros =
                        Some(connected.elapsed().as_micros() as u64);
                    break;
                }
            }
        }
        BluetoothPeripheralTermination::PeerRfkill => unreachable!(),
    }
    Ok(())
}

fn peer_disconnected(packet: &[u8], handle: ConnHandle, expected_reason: u8) -> Result<bool> {
    if packet.first() != Some(&4) {
        return Err("unexpected non-event HCI packet while awaiting target termination".into());
    }
    let (event, rest) = Event::from_hci_bytes(&packet[1..])
        .map_err(|error| format!("invalid target termination event: {error:?}"))?;
    if !rest.is_empty() {
        return Err("trailing target termination event bytes".into());
    }
    match event {
        Event::DisconnectionComplete(event) => {
            event
                .status
                .to_result()
                .map_err(|error| format!("peer disconnection failed: {error:?}"))?;
            if event.handle != handle || event.reason.into_inner() != expected_reason {
                return Err("peer disconnection does not match the target termination".into());
            }
            Ok(true)
        }
        Event::HardwareError(_) => {
            Err("controller hardware error awaiting target termination".into())
        }
        _ => Ok(false),
    }
}

fn connection_updated(
    packet: &[u8],
    handle: ConnHandle,
    interval: HciDuration<1_250>,
) -> Result<bool> {
    if packet.first() != Some(&4) {
        return Err("unexpected non-event HCI packet during connection update".into());
    }
    let (event, rest) = Event::from_hci_bytes(&packet[1..])
        .map_err(|error| format!("invalid connection update event: {error:?}"))?;
    if !rest.is_empty() {
        return Err("trailing connection update event bytes".into());
    }
    match event {
        Event::CommandStatus(status) if status.cmd_opcode == LeConnUpdate::OPCODE => {
            status
                .status
                .to_result()
                .map_err(|error| format!("Connection Update rejected: {error:?}"))?;
            Ok(false)
        }
        Event::Le(LeEvent::LeConnectionUpdateComplete(event)) => {
            event
                .status
                .to_result()
                .map_err(|error| format!("connection update failed: {error:?}"))?;
            if event.handle != handle
                || event.conn_interval != interval
                || event.peripheral_latency != 0
                || event.supervision_timeout != HciDuration::from_millis(2_000)
            {
                return Err("connection update completion does not match the request".into());
            }
            Ok(true)
        }
        Event::HardwareError(_) => Err("controller hardware error during connection update".into()),
        Event::DisconnectionComplete(_) => {
            Err("connection ended before the connection update completed".into())
        }
        _ => Ok(false),
    }
}

fn connection_created(packet: &[u8], peer: PeerAddress) -> Result<Option<ConnHandle>> {
    if packet.first() != Some(&4) {
        return Err("unexpected non-event HCI packet".into());
    }
    let (event, rest) = Event::from_hci_bytes(&packet[1..])
        .map_err(|error| format!("invalid connection event: {error:?}"))?;
    if !rest.is_empty() {
        return Err("trailing connection event bytes".into());
    }
    match event {
        Event::CommandStatus(status) if status.cmd_opcode == LeCreateConn::OPCODE => {
            status
                .status
                .to_result()
                .map_err(|error| format!("Create Connection rejected: {error:?}"))?;
            Ok(None)
        }
        Event::Le(LeEvent::LeConnectionComplete(event)) => {
            event
                .status
                .to_result()
                .map_err(|error| format!("connection failed: {error:?}"))?;
            if event.role != LeConnRole::Central
                || event.peer_addr_kind != AddrKind::PUBLIC
                || event.peer_addr != BdAddr::new(peer.0)
                || event.conn_interval != HciDuration::from_millis(100)
                || event.peripheral_latency != 0
                || event.supervision_timeout != HciDuration::from_millis(2_000)
            {
                return Err(
                    "connection completion does not match the requested peer and parameters".into(),
                );
            }
            Ok(Some(event.handle))
        }
        Event::HardwareError(_) => Err("controller hardware error during connection".into()),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completion(peer: PeerAddress) -> Vec<u8> {
        let mut packet = vec![4, 0x3e, 19, 1, 0, 1, 0, 0, 0];
        packet.extend(peer.0);
        packet.extend([80, 0, 0, 0, 200, 0, 0]);
        packet
    }

    #[test]
    fn only_the_exact_successful_connection_can_trigger_reset() {
        let peer = "30:ED:A0:F3:F6:D1".parse().unwrap();
        let packet = completion(peer);
        assert_eq!(
            connection_created(&packet, peer).unwrap(),
            Some(ConnHandle::new(1))
        );
        for offset in [4, 7, 8, 9, 15, 17, 19] {
            let mut wrong = packet.clone();
            wrong[offset] ^= 1;
            assert!(connection_created(&wrong, peer).is_err(), "offset {offset}");
        }
        assert_eq!(
            connection_created(&[4, 0x0f, 4, 0, 1, 0x0d, 0x20], peer).unwrap(),
            None
        );
        assert!(connection_created(&[4, 0x0f, 4, 0x0c, 1, 0x0d, 0x20], peer).is_err());
        assert!(connection_created(&packet[..packet.len() - 1], peer).is_err());
        let mut extra = packet;
        extra.push(0);
        assert!(connection_created(&extra, peer).is_err());
    }

    #[test]
    fn acl_echo_requires_the_exact_ordered_fragment_sequence() {
        let handle = ConnHandle::new(1);
        let mut assembly = AclEchoAssembly::new();
        for (index, fragment) in ACL_ECHO_PAYLOAD.chunks(27).enumerate() {
            let acl = AclPacket::new(
                handle,
                if index == 0 {
                    AclPacketBoundary::FirstFlushable
                } else {
                    AclPacketBoundary::Continuing
                },
                AclBroadcastFlag::PointToPoint,
                fragment,
            );
            let mut packet = vec![2; 1 + bt_hci::WriteHci::size(&acl)];
            bt_hci::WriteHci::write_hci(&acl, &mut packet[1..]).unwrap();
            assert_eq!(
                assembly
                    .receive(&packet, handle, &ACL_ECHO_PAYLOAD)
                    .unwrap(),
                index + 1 == ACL_ECHO_PAYLOAD.len().div_ceil(27)
            );
        }
        assert_eq!(assembly.bytes, ACL_ECHO_PAYLOAD);
        assert_eq!(assembly.hci_packets, 10);

        let stale = AclPacket::new(
            handle,
            AclPacketBoundary::FirstFlushable,
            AclBroadcastFlag::PointToPoint,
            &ACL_ECHO_PAYLOAD[..27],
        );
        let mut stale_packet = vec![2; 1 + bt_hci::WriteHci::size(&stale)];
        bt_hci::WriteHci::write_hci(&stale, &mut stale_packet[1..]).unwrap();
        assert!(
            AclEchoAssembly::new()
                .receive(&stale_packet, handle, &POST_UPDATE_ACL_ECHO_PAYLOAD)
                .is_err()
        );

        let mut corrupt = AclEchoAssembly::new();
        let acl = AclPacket::new(
            handle,
            AclPacketBoundary::FirstFlushable,
            AclBroadcastFlag::PointToPoint,
            &ACL_ECHO_PAYLOAD[..27],
        );
        let mut packet = vec![2; 1 + bt_hci::WriteHci::size(&acl)];
        bt_hci::WriteHci::write_hci(&acl, &mut packet[1..]).unwrap();
        packet[5] ^= 1;
        assert!(corrupt.receive(&packet, handle, &ACL_ECHO_PAYLOAD).is_err());
        assert!(
            !AclEchoAssembly::new()
                .receive(&[4, 0x13, 1, 0], handle, &ACL_ECHO_PAYLOAD)
                .unwrap()
        );
        assert!(
            AclEchoAssembly::new()
                .receive(&[2, 1, 0x20, 12, 0], handle, &ACL_ECHO_PAYLOAD)
                .is_err()
        );
    }

    #[test]
    fn connection_update_requires_the_exact_successful_parameters() {
        let handle = ConnHandle::new(1);
        let interval =
            HciDuration::from_millis(u32::from(BLUETOOTH_PERIPHERAL_UPDATED_INTERVAL_MILLIS));
        assert!(!connection_updated(&[4, 15, 4, 0, 1, 0x13, 0x20], handle, interval).unwrap());
        let complete = [4, 0x3e, 10, 3, 0, 1, 0, 96, 0, 0, 0, 200, 0];
        assert!(connection_updated(&complete, handle, interval).unwrap());
        for offset in [4, 5, 7, 9, 11] {
            let mut wrong = complete;
            wrong[offset] ^= 1;
            assert!(connection_updated(&wrong, handle, interval).is_err());
        }
        assert!(connection_updated(&[4, 15, 4, 0x0c, 1, 0x13, 0x20], handle, interval).is_err());
    }

    #[test]
    fn channel_map_comparison_requires_all_37_data_channels() {
        let mut expected = ChannelMap::new();
        for channel in 2..37 {
            expected.set_channel_bad(channel, true);
        }
        assert!(channel_maps_match(&expected, &expected));
        for channel in [0, 1, 2, 36] {
            let mut different = expected;
            different.set_channel_bad(channel, !expected.is_channel_bad(channel));
            assert!(!channel_maps_match(&different, &expected));
        }
    }

    #[test]
    fn target_termination_requires_the_exact_handle_status_and_reason() {
        let handle = ConnHandle::new(1);
        let complete = [4, 5, 4, 0, 1, 0, 0x13];
        assert!(peer_disconnected(&complete, handle, 0x13).unwrap());
        for offset in [3, 4, 6] {
            let mut wrong = complete;
            wrong[offset] ^= 1;
            assert!(peer_disconnected(&wrong, handle, 0x13).is_err());
        }
        assert!(peer_disconnected(&complete, handle, 0x08).is_err());
        assert!(!peer_disconnected(&[4, 0x13, 1, 0], handle, 0x13).unwrap());
    }
}

fn enable_encryption(
    user: &Socket,
    handle: ConnHandle,
    report: &mut ConnectionReset,
    refresh: bool,
) -> Result<()> {
    use oer_hil_protocol::{
        bluetooth::BLUETOOTH_REFRESH_EDIV, bluetooth::BLUETOOTH_REFRESH_LTK,
        bluetooth::BLUETOOTH_REFRESH_RAND, bluetooth::BLUETOOTH_TEST_EDIV,
        bluetooth::BLUETOOTH_TEST_LTK, bluetooth::BLUETOOTH_TEST_RAND,
    };
    let started = Instant::now();
    user.send_command(&LeEnableEncryption::new(
        handle,
        if refresh {
            BLUETOOTH_REFRESH_RAND
        } else {
            BLUETOOTH_TEST_RAND
        },
        if refresh {
            BLUETOOTH_REFRESH_EDIV
        } else {
            BLUETOOTH_TEST_EDIV
        },
        if refresh {
            BLUETOOTH_REFRESH_LTK
        } else {
            BLUETOOTH_TEST_LTK
        },
    ))?;
    let deadline = started + Duration::from_secs(5);
    loop {
        let packet = user.receive(deadline)?;
        if observe_encryption_event(&packet, handle, report, refresh)? {
            let elapsed = Some(started.elapsed().as_micros().try_into()?);
            if refresh {
                report.refresh_after_micros = elapsed;
            } else {
                report.encryption_after_micros = elapsed;
            }
            return Ok(());
        }
    }
}

fn observe_encryption_event(
    packet: &[u8],
    handle: ConnHandle,
    report: &mut ConnectionReset,
    refresh: bool,
) -> Result<bool> {
    if packet.first() != Some(&4) {
        return Err("unexpected data before encryption completed".into());
    }
    let (event, rest) =
        Event::from_hci_bytes(&packet[1..]).map_err(|e| format!("encryption event: {e:?}"))?;
    if !rest.is_empty() {
        return Err("trailing encryption event bytes".into());
    }
    match event {
        Event::CommandStatus(status) if status.cmd_opcode == LeEnableEncryption::OPCODE => {
            status
                .status
                .to_result()
                .map_err(|e| format!("encryption command: {e:?}"))?;
            let accepted = if refresh {
                &mut report.refresh_command_status
            } else {
                &mut report.encryption_command_status
            };
            if *accepted {
                return Err("duplicate encryption command status".into());
            }
            *accepted = true;
        }
        Event::EncryptionChangeV1(change) => {
            change
                .status
                .to_result()
                .map_err(|e| format!("encryption failed: {e:?}"))?;
            if refresh
                || report.encryption_change
                || !report.encryption_command_status
                || change.handle != handle
                || change.enabled != bt_hci::param::EncryptionEnabledLevel::OnE0OrAesCcm
            {
                return Err(
                    "encryption completion does not match active AES-CCM connection".into(),
                );
            }
            report.encryption_change = true;
            return Ok(true);
        }
        Event::EncryptionKeyRefreshComplete(complete) => {
            if !refresh
                || !report.encryption_change
                || !report.refresh_command_status
                || report.key_refresh_complete
                || complete.handle != handle
                || complete.status != bt_hci::param::Status::SUCCESS
            {
                return Err(format!(
                    "key refresh completion mismatch: status=0x{:02x}, handle={}, expected={}, refresh={}, initial_encrypted={}, admitted={}, duplicate={}",
                    complete.status.into_inner(), complete.handle.raw(), handle.raw(),
                    refresh, report.encryption_change, report.refresh_command_status, report.key_refresh_complete,
                ).into());
            }
            report.key_refresh_complete = true;
            return Ok(true);
        }
        Event::DisconnectionComplete(_) | Event::HardwareError(_) => {
            return Err("link failed while enabling encryption".into());
        }
        _ => {}
    }
    Ok(false)
}

#[cfg(test)]
mod encryption_tests {
    use super::*;
    #[test]
    fn refresh_requires_prior_encryption_new_status_and_exact_refresh_event() {
        let handle = ConnHandle::new(1);
        let status = [4, 0x0f, 4, 0, 1, 0x19, 0x20];
        let refreshed = [4, 0x30, 3, 0, 1, 0];
        let fresh = || {
            ConnectionReset::new(
                super::super::model::Adapter(0),
                PeerAddress([1; 6]),
                0,
                BluetoothPeripheralTermination::PeerReset,
            )
        };
        let mut report = fresh();
        observe_encryption_event(&status, handle, &mut report, true).unwrap();
        assert!(observe_encryption_event(&refreshed, handle, &mut report, true).is_err());
        for packet in [
            vec![4, 0x30, 3, 0, 2, 0],             // wrong connection
            vec![4, 0x30, 3, 0x06, 1, 0],          // failed refresh
            vec![4, 0x30, 3, 0, 1, 0, 0],          // trailing bytes
            vec![4, 0x08, 4, 0, 1, 0, 1], // a second initial Encryption Change is not refresh
            vec![2, 1, 0, 0, 0],          // user data during refresh
            vec![4, 0x0f, 4, 0x0c, 1, 0x19, 0x20], // rejected command
        ] {
            let mut report = fresh();
            report.encryption_change = true;
            observe_encryption_event(&status, handle, &mut report, true).unwrap();
            assert!(observe_encryption_event(&packet, handle, &mut report, true).is_err());
            assert!(!report.key_refresh_complete);
        }
        let mut report = fresh();
        report.encryption_change = true;
        assert!(observe_encryption_event(&refreshed, handle, &mut report, true).is_err());
        observe_encryption_event(&status, handle, &mut report, true).unwrap();
        assert!(observe_encryption_event(&status, handle, &mut report, true).is_err());
        assert!(observe_encryption_event(&refreshed, handle, &mut report, false).is_err());
        assert!(observe_encryption_event(&refreshed, handle, &mut report, true).unwrap());
        assert!(report.key_refresh_complete);
        assert!(observe_encryption_event(&refreshed, handle, &mut report, true).is_err());
    }

    #[test]
    fn refresh_failure_preserves_exact_peer_status_and_handle_in_error() {
        for status in [0x06, 0x08, 0x3d] {
            let mut report = ConnectionReset::new(
                super::super::model::Adapter(0),
                PeerAddress([1; 6]),
                0,
                BluetoothPeripheralTermination::PeerReset,
            );
            report.encryption_change = true;
            report.refresh_command_status = true;
            let error = observe_encryption_event(
                &[4, 0x30, 3, status, 1, 0],
                ConnHandle::new(1),
                &mut report,
                true,
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains(&format!("status=0x{status:02x}, handle=1, expected=1")));
            assert!(!report.key_refresh_complete);
        }
    }

    #[test]
    fn encryption_requires_ordered_status_and_exact_successful_handle_and_mode() {
        let handle = ConnHandle::new(1);
        let status = [4, 0x0f, 4, 0, 1, 0x19, 0x20];
        let complete = [4, 0x08, 4, 0, 1, 0, 1];
        let fresh = || {
            ConnectionReset::new(
                super::super::model::Adapter(0),
                PeerAddress([1; 6]),
                0,
                BluetoothPeripheralTermination::PeerReset,
            )
        };
        assert!(observe_encryption_event(&complete, handle, &mut fresh(), false).is_err());
        let mut report = fresh();
        assert!(!observe_encryption_event(&status, handle, &mut report, false).unwrap());
        assert!(observe_encryption_event(&status, handle, &mut report, false).is_err());
        assert!(observe_encryption_event(&complete, handle, &mut report, false).unwrap());
        assert!(report.encryption_change);
        for packet in [
            vec![4, 0x08, 4, 0, 2, 0, 1],
            vec![4, 0x08, 4, 0, 1, 0, 0],
            vec![4, 0x08, 4, 0x06, 1, 0, 1],
            vec![2, 1, 0, 0, 0],
            vec![4, 0x08, 4, 0, 1, 0, 1, 0],
        ] {
            let mut report = fresh();
            observe_encryption_event(&status, handle, &mut report, false).unwrap();
            assert!(observe_encryption_event(&packet, handle, &mut report, false).is_err());
            assert!(!report.encryption_change);
        }
    }
}
