//! Linux HCI sockets with bounded, cancellable command completion.
//! HCI payloads use bt-hci; only Linux management and H4 framing live here.

use super::Result;
use bt_hci::{
    FromHciBytes,
    cmd::SyncCmd,
    event::{CommandCompleteWithStatus, Event},
};
use oer_bluetooth_hci::h4;
use oer_os_process::linux_socket::HciAddress;
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
        oer_os_process::check_cancelled()?;
        let count = send(&self.0, bytes, SendFlags::NOSIGNAL)?;
        if count != bytes.len() {
            return Err("short HCI socket write".into());
        }
        Ok(())
    }

    pub(super) fn receive(&self, deadline: Instant) -> Result<Vec<u8>> {
        loop {
            oer_os_process::check_cancelled()?;
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
        let bytes = h4_frame(bt_hci::PacketKind::Cmd, command)
            .map_err(|error| format!("HCI encode: {error}"))?;
        self.send(&bytes)
    }

    pub(super) fn send_acl(&self, packet: &bt_hci::data::AclPacket<'_>) -> Result<()> {
        let bytes = h4_frame(bt_hci::PacketKind::AclData, packet)
            .map_err(|error| format!("HCI ACL encode: {error}"))?;
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
/// The H4 frame of an encodable packet: the indicator of `kind` and the packet.
fn h4_frame(kind: bt_hci::PacketKind, packet: &impl bt_hci::WriteHci) -> Result<Vec<u8>> {
    let mut bytes = vec![0; 1 + bt_hci::WriteHci::size(packet)];
    let (indicator, body) = bytes.split_first_mut().ok_or("empty H4 frame")?;
    *indicator = h4::indicator(kind);
    bt_hci::WriteHci::write_hci(packet, body).map_err(|error| format!("{error:?}"))?;
    Ok(bytes)
}

/// The HCI event after the H4 event indicator; `None` for any other frame.
pub(super) fn event_body(packet: &[u8]) -> Option<&[u8]> {
    match h4::split(packet) {
        Ok((bt_hci::PacketKind::Event, body)) => Some(body),
        _ => None,
    }
}

fn command_event_summary(packet: &[u8]) -> String {
    let Some(body) = event_body(packet) else {
        return format!("non-event H4 kind {:?}", packet.first());
    };
    match Event::from_hci_bytes(body) {
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
    let body = event_body(packet).ok_or("unexpected non-event HCI packet")?;
    let (event, rest) =
        Event::from_hci_bytes(body).map_err(|error| format!("invalid HCI event: {error:?}"))?;
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
