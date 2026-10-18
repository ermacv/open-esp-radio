//! Linux HCI sockets with bounded, cancellable command completion.
//! HCI payloads use bt-hci; only Linux management and H4 framing live here.

use super::Result;
use bt_hci::{
    FromHciBytes,
    cmd::SyncCmd,
    event::{CommandCompleteWithStatus, Event},
};
use oer_hil_fixture::linux_socket::HciAddress;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    net::{
        AddressFamily, Protocol, RecvFlags, SendFlags, SocketFlags, SocketType, bind, recv, send,
        socket_with,
    },
};
use std::{
    num::NonZeroU32,
    os::fd::OwnedFd,
    time::{Duration, Instant},
};

/// Linux `BTPROTO_HCI`.
const HCI_PROTOCOL: Protocol = Protocol::from_raw(NonZeroU32::new(1).unwrap());

pub(super) struct Socket(pub(super) OwnedFd);

#[derive(Debug)]
pub(super) struct ManagementError {
    opcode: u16,
    pub(super) status: u8,
}
impl std::fmt::Display for ManagementError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            output,
            "management {:#06x}: status {:#04x}",
            self.opcode, self.status
        )
    }
}
impl std::error::Error for ManagementError {}

impl Socket {
    pub(super) fn open(address: HciAddress) -> Result<Self> {
        let socket = Self(socket_with(
            AddressFamily::BLUETOOTH,
            SocketType::RAW,
            SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
            Some(HCI_PROTOCOL),
        )?);
        bind(&socket.0, &address)?;
        Ok(socket)
    }

    pub(super) fn send(&self, bytes: &[u8]) -> Result<()> {
        oer_process::check_cancelled()?;
        let count = send(&self.0, bytes, SendFlags::NOSIGNAL)?;
        if count != bytes.len() {
            return Err("short HCI socket write".into());
        }
        Ok(())
    }

    pub(super) fn receive(&self, deadline: Instant) -> Result<Vec<u8>> {
        loop {
            oer_process::check_cancelled()?;
            if Instant::now() >= deadline {
                return Err("HCI response timeout".into());
            }
            let mut bytes = vec![0; 4096];
            match recv(&self.0, &mut bytes[..], RecvFlags::TRUNC) {
                Ok((_, length)) => {
                    if length == 0 || length > bytes.len() {
                        return Err("empty or truncated HCI packet".into());
                    }
                    bytes.truncate(length);
                    return Ok(bytes);
                }
                Err(Errno::WOULDBLOCK | Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
            // Wake as soon as a connection event arrives, while keeping signal
            // cancellation bounded. A periodic sleep can miss the first window.
            let remaining = deadline.saturating_duration_since(Instant::now());
            let timeout = Timespec::try_from(
                remaining.clamp(Duration::from_millis(1), Duration::from_millis(10)),
            )?;
            match poll(&mut [PollFd::new(&self.0, PollFlags::IN)], Some(&timeout)) {
                Ok(_) | Err(Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub(super) fn send_command<C: bt_hci::cmd::Cmd>(&self, command: &C) -> Result<()> {
        let mut bytes = vec![0; 1 + bt_hci::WriteHci::size(command)];
        bytes[0] = 1;
        bt_hci::WriteHci::write_hci(command, &mut bytes[1..])
            .map_err(|error| format!("HCI encode: {error:?}"))?;
        self.send(&bytes)
    }

    pub(super) fn send_acl(&self, packet: &bt_hci::data::AclPacket<'_>) -> Result<()> {
        let mut bytes = vec![0; 1 + bt_hci::WriteHci::size(packet)];
        bytes[0] = bt_hci::PacketKind::AclData as u8;
        bt_hci::WriteHci::write_hci(packet, &mut bytes[1..])
            .map_err(|error| format!("HCI ACL encode: {error:?}"))?;
        self.send(&bytes)
    }

    pub(super) fn command<C: SyncCmd>(&self, command: C) -> Result<C::Return> {
        self.command_with_timeout(command, Duration::from_secs(2))
    }

    fn command_with_timeout<C: SyncCmd>(&self, command: C, timeout: Duration) -> Result<C::Return> {
        let started = Instant::now();
        self.send_command(&command)?;
        let deadline = started + timeout;
        let mut received_packets = 0_u64;
        let mut last_event = String::from("none");
        loop {
            let packet = self.receive(deadline).map_err(|error| {
                format!(
                    "HCI {:?} ({}): {error}; elapsed={} us, received_packets={received_packets}, last_event={last_event}",
                    C::OPCODE,
                    std::any::type_name::<C>(),
                    started.elapsed().as_micros(),
                )
            })?;
            received_packets = received_packets.saturating_add(1);
            last_event = command_event_summary(&packet);
            if let Some(value) = completion::<C>(&packet)? {
                return Ok(value);
            }
        }
    }

    pub(super) fn management(&self, index: u16, opcode: u16, payload: &[u8]) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        bytes.extend(opcode.to_le_bytes());
        bytes.extend(index.to_le_bytes());
        bytes.extend((payload.len() as u16).to_le_bytes());
        bytes.extend(payload);
        self.send(&bytes)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let packet = self.receive(deadline)?;
            if let Some(value) = management_completion(&packet, index, opcode)? {
                return Ok(value);
            }
        }
    }
}

/// Retain only the last event's identity, never its payload: this socket also
/// carries pairing keys and ACL data. Unrelated traffic cannot grow diagnostic
/// storage or extend the original command deadline.
fn command_event_summary(packet: &[u8]) -> String {
    if packet.first() != Some(&4) {
        return format!("non-event H4 kind {:?}", packet.first());
    }
    match Event::from_hci_bytes(&packet[1..]) {
        Ok((Event::CommandComplete(event), _)) => {
            format!("CommandComplete {:?}", event.cmd_opcode)
        }
        Ok((Event::CommandStatus(event), _)) => {
            format!("CommandStatus {:?} {:?}", event.cmd_opcode, event.status)
        }
        _ => format!("event code {:?}", packet.get(1)),
    }
}

fn completion<C: SyncCmd>(packet: &[u8]) -> Result<Option<C::Return>> {
    if packet.first() != Some(&4) {
        return Err("unexpected non-event HCI packet".into());
    }
    let (event, rest) = Event::from_hci_bytes(&packet[1..])
        .map_err(|error| format!("invalid HCI event: {error:?}"))?;
    if !rest.is_empty() {
        return Err("trailing HCI event bytes".into());
    }
    match event {
        Event::CommandComplete(event) if event.cmd_opcode == C::OPCODE => {
            let event = CommandCompleteWithStatus::try_from(event)
                .map_err(|error| format!("HCI completion: {error:?}"))?;
            event
                .to_result::<C>()
                .map(Some)
                .map_err(|error| format!("HCI {:?} rejected: {error:?}", C::OPCODE).into())
        }
        Event::CommandStatus(event) if event.cmd_opcode == C::OPCODE => {
            event
                .status
                .to_result()
                .map_err(|error| format!("HCI {:?} status: {error:?}", C::OPCODE))?;
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn management_completion(packet: &[u8], index: u16, opcode: u16) -> Result<Option<Vec<u8>>> {
    if packet.len() < 6 {
        return Err("short management header".into());
    }
    let read = |offset| u16::from_le_bytes([packet[offset], packet[offset + 1]]);
    if packet.len() != 6 + usize::from(read(4)) {
        return Err("invalid management length".into());
    }
    if read(2) != index || !matches!(read(0), 1 | 2) {
        return Ok(None);
    }
    if packet.len() < 9 {
        return Err("short management completion".into());
    }
    if read(6) != opcode {
        return Ok(None);
    }
    if packet[8] != 0 {
        return Err(ManagementError {
            opcode,
            status: packet[8],
        }
        .into());
    }
    if read(0) == 2 {
        return Ok(None);
    }
    Ok(Some(packet[9..].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_hci::cmd::le::LeTestEnd;
    #[test]
    fn connect_reset_drives_the_compiled_command_sequence_without_hardware() {
        let (client, server) = std::os::unix::net::UnixDatagram::pair().unwrap();
        client.set_nonblocking(true).unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let peer = super::super::model::PeerAddress([0xd1, 0xf6, 0xf3, 0xa0, 0xed, 0x30]);
        let worker = std::thread::spawn(move || {
            let mut bytes = [0; 300];
            for opcode in [0x0c01_u16, 0x2001] {
                assert!(server.recv(&mut bytes).unwrap() >= 4);
                assert_eq!(&bytes[..3], &[1, opcode as u8, (opcode >> 8) as u8]);
                server
                    .send(&[4, 14, 4, 1, opcode as u8, (opcode >> 8) as u8, 0])
                    .unwrap();
            }
            assert!(server.recv(&mut bytes).unwrap() >= 4);
            assert_eq!(&bytes[..3], &[1, 0x0d, 0x20]);
            server.send(&[4, 15, 4, 0, 1, 0x0d, 0x20]).unwrap();
            let mut event = vec![4, 0x3e, 19, 1, 0, 1, 0, 0, 0];
            event.extend(peer.0);
            event.extend([80, 0, 0, 0, 200, 0, 0]);
            server.send(&event).unwrap();
            assert_eq!(server.recv(&mut bytes).unwrap(), 6);
            assert_eq!(&bytes[..6], &[1, 0x16, 0x20, 2, 1, 0]);
            server.send(&[4, 0x0f, 4, 0, 1, 0x16, 0x20]).unwrap();
            // Exercise the production LL response against the independent helper
            // profile, so adding an advertised procedure cannot silently drift.
            let mut control = oer_bluetooth_ll::control::LePeripheralControl::new();
            control
                .receive(&[3, 9, 8, 255, 255, 255, 255, 255, 255, 255, 255], None)
                .unwrap();
            let response = control.pending_response().unwrap();
            let mut features_complete = vec![4, 0x3e, 12, 4, 0, 1, 0];
            features_complete.extend_from_slice(&response.as_bytes()[1..]);
            server.send(&features_complete).unwrap();
            assert_eq!(server.recv(&mut bytes).unwrap(), 6);
            assert_eq!(&bytes[..6], &[1, 0x1d, 0x04, 2, 1, 0]);
            server.send(&[4, 0x0f, 4, 0, 1, 0x1d, 0x04]).unwrap();
            server
                .send(&[4, 0x0c, 8, 0, 1, 0, 0x0d, 0xff, 0xff, 1, 0])
                .unwrap();
            let count = server.recv(&mut bytes).unwrap();
            let payload = super::super::connection_reset::ACL_ECHO_PAYLOAD;
            assert_eq!(count, 1 + 4 + payload.len());
            assert_eq!(bytes[0], bt_hci::PacketKind::AclData as u8);
            let (acl, rest) = bt_hci::data::AclPacket::from_hci_bytes(&bytes[1..count]).unwrap();
            assert!(rest.is_empty());
            assert_eq!(acl.handle(), bt_hci::param::ConnHandle::new(1));
            assert_eq!(
                acl.boundary_flag(),
                bt_hci::data::AclPacketBoundary::FirstNonFlushable
            );
            assert_eq!(acl.data(), payload);
            for (index, fragment) in payload.chunks(27).enumerate() {
                let echo = bt_hci::data::AclPacket::new(
                    bt_hci::param::ConnHandle::new(1),
                    if index == 0 {
                        bt_hci::data::AclPacketBoundary::FirstFlushable
                    } else {
                        bt_hci::data::AclPacketBoundary::Continuing
                    },
                    bt_hci::data::AclBroadcastFlag::PointToPoint,
                    fragment,
                );
                let mut encoded = vec![0; 1 + bt_hci::WriteHci::size(&echo)];
                encoded[0] = bt_hci::PacketKind::AclData as u8;
                bt_hci::WriteHci::write_hci(&echo, &mut encoded[1..]).unwrap();
                server.send(&encoded).unwrap();
            }
            assert_eq!(server.recv(&mut bytes).unwrap(), 18);
            assert_eq!(
                &bytes[..18],
                &[
                    1, 0x13, 0x20, 14, 1, 0, 96, 0, 96, 0, 0, 0, 200, 0, 0, 0, 0, 0
                ]
            );
            server.send(&[4, 15, 4, 0, 1, 0x13, 0x20]).unwrap();
            server
                .send(&[4, 0x3e, 10, 3, 0, 1, 0, 96, 0, 0, 0, 200, 0])
                .unwrap();
            assert_eq!(server.recv(&mut bytes).unwrap(), 9);
            assert_eq!(&bytes[..9], &[1, 0x14, 0x20, 5, 3, 0, 0, 0, 0]);
            server.send(&[4, 14, 4, 1, 0x14, 0x20, 0]).unwrap();
            assert_eq!(server.recv(&mut bytes).unwrap(), 6);
            assert_eq!(&bytes[..6], &[1, 0x15, 0x20, 2, 1, 0]);
            server
                .send(&[4, 14, 11, 1, 0x15, 0x20, 0, 1, 0, 3, 0, 0, 0, 0])
                .unwrap();
            let payload = super::super::connection_reset::POST_UPDATE_ACL_ECHO_PAYLOAD;
            let count = server.recv(&mut bytes).unwrap();
            assert_eq!(count, 1 + 4 + payload.len());
            assert_eq!(bytes[0], bt_hci::PacketKind::AclData as u8);
            let (acl, rest) = bt_hci::data::AclPacket::from_hci_bytes(&bytes[1..count]).unwrap();
            assert!(rest.is_empty());
            assert_eq!(acl.handle(), bt_hci::param::ConnHandle::new(1));
            assert_eq!(acl.data(), payload);
            for (index, fragment) in payload.chunks(27).enumerate() {
                let echo = bt_hci::data::AclPacket::new(
                    bt_hci::param::ConnHandle::new(1),
                    if index == 0 {
                        bt_hci::data::AclPacketBoundary::FirstFlushable
                    } else {
                        bt_hci::data::AclPacketBoundary::Continuing
                    },
                    bt_hci::data::AclBroadcastFlag::PointToPoint,
                    fragment,
                );
                let mut encoded = vec![0; 1 + bt_hci::WriteHci::size(&echo)];
                encoded[0] = bt_hci::PacketKind::AclData as u8;
                bt_hci::WriteHci::write_hci(&echo, &mut encoded[1..]).unwrap();
                server.send(&encoded).unwrap();
            }
            assert_eq!(server.recv(&mut bytes).unwrap(), 4);
            assert_eq!(&bytes[..4], &[1, 3, 12, 0]);
            server.send(&[4, 14, 4, 1, 3, 12, 0]).unwrap();
        });
        let mut report = super::super::model::ConnectionReset::new(
            super::super::model::Adapter(0),
            peer,
            0,
            oer_hil_protocol::bluetooth::BluetoothPeripheralTermination::PeerReset,
        );
        let outcome = super::super::connection_reset::run(
            &Socket(client.into()),
            peer,
            0,
            oer_hil_protocol::bluetooth::BluetoothPeripheralTermination::PeerReset,
            &mut report,
        )
        .unwrap();
        assert!(matches!(
            outcome,
            super::super::connection_reset::ConnectionRunOutcome::Complete
        ));
        worker.join().unwrap();
        assert!(report.connection_complete && report.reset_completed);
        assert!(report.remote_features_command_status && report.remote_features_complete);
        assert_eq!(report.remote_features, Some([0x39, 0x40, 0, 0, 0, 0, 0, 0]));
        assert!(report.remote_features_after_micros.is_some());
        assert!(report.remote_version_command_status && report.remote_version_complete);
        assert_eq!(report.remote_version, Some(0x0d));
        assert_eq!(report.remote_version_company, Some(0xffff));
        assert_eq!(report.remote_version_subversion, Some(1));
        assert!(report.remote_version_after_micros.is_some());
        assert!(report.acl_sent && report.acl_echo_received);
        assert_eq!(
            report.acl_payload_bytes,
            Some(oer_hil_protocol::bluetooth::BLUETOOTH_PERIPHERAL_ACL_PAYLOAD_BYTES as u16)
        );
        assert_eq!(report.acl_echo_hci_packets, Some(10));
        assert!(report.acl_echo_after_micros.is_some());
        assert!(report.connection_update_complete);
        assert_eq!(report.updated_interval_millis, Some(120));
        assert!(report.connection_update_after_micros.is_some());
        assert!(report.channel_map_updated);
        assert!(report.channel_map_update_after_micros.is_some());
        assert!(report.post_update_acl_sent && report.post_update_acl_echo_received);
        assert_eq!(report.post_update_acl_echo_hci_packets, Some(10));
        assert!(report.post_update_acl_echo_after_micros.is_some());
        assert!(report.termination_after_connection_micros.is_some());
        assert!(!report.restored);
    }
    #[test]
    fn socket_exchange_and_expired_deadline_are_bounded_without_hardware() {
        let (client, server) = std::os::unix::net::UnixDatagram::pair().unwrap();
        client.set_nonblocking(true).unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let peer = std::thread::spawn(move || {
            let mut request = [0; 64];
            let count = server.recv(&mut request).unwrap();
            assert!(count > 1 && request[0] == 1);
            server.send(&[4, 14, 6, 1, 0x1f, 0x20, 0, 42, 0]).unwrap();
        });
        let socket = Socket(client.into());
        assert_eq!(socket.command(LeTestEnd::new()).unwrap(), 42);
        peer.join().unwrap();
        assert!(
            socket
                .receive(Instant::now())
                .unwrap_err()
                .to_string()
                .contains("timeout")
        );
    }
    #[test]
    fn receiver_count_requires_matching_successful_complete_packet() {
        assert_eq!(
            completion::<LeTestEnd>(&[4, 14, 6, 1, 0x1f, 0x20, 0, 42, 0]).unwrap(),
            Some(42)
        );
        assert!(
            completion::<LeTestEnd>(&[4, 14, 6, 1, 0x1e, 0x20, 0, 42, 0])
                .unwrap()
                .is_none()
        );
        assert!(completion::<LeTestEnd>(&[4, 14, 4, 1, 0x1f, 0x20, 0x0c]).is_err());
        assert!(completion::<LeTestEnd>(&[4, 14, 6, 1, 0x1f, 0x20, 0, 42]).is_err());
        assert!(completion::<LeTestEnd>(&[4, 14, 6, 1, 0x1f, 0x20, 0, 42, 0, 0]).is_err());
        assert!(completion::<LeTestEnd>(&[4, 15, 4, 0x0c, 1, 0x1f, 0x20]).is_err());
    }
    #[test]
    fn missing_completion_reports_command_and_unrelated_event_without_retry() {
        let (client, server) = std::os::unix::net::UnixDatagram::pair().unwrap();
        client.set_nonblocking(true).unwrap();
        server.set_nonblocking(true).unwrap();
        // A successful status does not complete Test End, nor does a Reset
        // completion. Neither may renew the original deadline or trigger retry.
        server.send(&[4, 15, 4, 0, 1, 0x1f, 0x20]).unwrap();
        server.send(&[4, 14, 4, 1, 3, 12, 0]).unwrap();
        let error = Socket(client.into())
            .command_with_timeout(LeTestEnd::new(), Duration::from_millis(20))
            .unwrap_err()
            .to_string();
        assert!(error.contains("LeTestEnd"), "{error}");
        assert!(error.contains("HCI response timeout"), "{error}");
        assert!(error.contains("received_packets=2"), "{error}");
        assert!(error.contains("last_event=CommandComplete"), "{error}");
        let mut request = [0; 64];
        assert_eq!(server.recv(&mut request).unwrap(), 4);
        assert_eq!(&request[..4], &[1, 0x1f, 0x20, 0]);
        assert_eq!(
            server.recv(&mut request).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn command_diagnostics_do_not_include_event_payloads() {
        // Arbitrary vendor event payloads can contain private controller data.
        let secret = b"private-controller-data";
        let mut event = vec![4, 0xff, secret.len() as u8];
        event.extend_from_slice(secret);
        assert_eq!(command_event_summary(&event), "event code Some(255)");
        // CommandComplete's return parameters are not logged either.
        let mut event = vec![4, 14, (3 + secret.len()) as u8, 1, 3, 12];
        event.extend_from_slice(secret);
        let summary = command_event_summary(&event);
        assert!(summary.starts_with("CommandComplete"), "{summary}");
        assert!(!summary.contains("private"), "{summary}");
    }
    #[test]
    fn management_ignores_other_adapters_and_checks_status_and_length() {
        let packet = [1, 0, 0, 0, 4, 0, 4, 0, 0, 123];
        assert_eq!(
            management_completion(&packet, 0, 4).unwrap(),
            Some(vec![123])
        );
        assert!(management_completion(&packet, 1, 4).unwrap().is_none());
        assert!(management_completion(&packet, 0, 5).unwrap().is_none());
        let mut rejected = packet;
        rejected[8] = 0x0a;
        assert!(management_completion(&rejected, 0, 4).is_err());
        assert!(management_completion(&packet[..9], 0, 4).is_err());
    }
}
