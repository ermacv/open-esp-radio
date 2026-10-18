//! Driver of the Thread reference peer.
//!
//! The peer runs the OpenThread peer project of `hil/peers`: ESP-IDF's
//! OpenThread stack behind a line protocol on its console (see that README).
//! It shares the board and the peer console ([`oer_hil_link::peer`]) with
//! the IEEE 802.15.4 peer. Commands are answered by `@OK`/`@ERR`; received
//! datagrams arrive at any time and are queued while a command waits for its
//! answer.

use std::{net::Ipv6Addr, path::Path, time::Duration};

use oer_device_peer_line::{Report, hex, to_hex};
use oer_hil_link::peer::{
    Expected, PeerConsole, PeerLink, PeerTranscript, RecordingLink, SerialLink,
};

use crate::Result;

/// Protocol version the driver speaks.
pub const THREAD_PEER_PROTOCOL: u32 = 1;
/// Board-journal image name of the OpenThread peer project of `hil/peers`.
pub const THREAD_PEER_IMAGE: &str = "openthread-peer";
/// How to restore the peer firmware when another consumer replaced it.
pub const THREAD_PEER_REFLASH: &str =
    "restore it with `cargo hil firmware flash openthread-peer --board <peer board>`";
/// OpenThread starts its task before the peer reports ready.
const READY_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// The peer's view of its Thread interface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadPeerState {
    /// OpenThread's device role name: `disabled`, `detached`, `child`,
    /// `router` or `leader`.
    pub role: String,
    pub rloc16: u16,
    /// The mesh-local EID.
    pub eid: Ipv6Addr,
}

/// One datagram the peer's socket received.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadDatagram {
    pub source: Ipv6Addr,
    pub port: u16,
    pub payload: Vec<u8>,
}

/// The peer's state, from its `@STATE` report.
fn state(report: &Report) -> Option<ThreadPeerState> {
    Some(ThreadPeerState {
        role: report.field("role")?.to_owned(),
        rloc16: u16::from_str_radix(report.field("rloc16")?, 16).ok()?,
        eid: report.field("eid")?.parse().ok()?,
    })
}

/// The datagram of an `@UDPRX` report.
fn datagram(report: &Report) -> Option<ThreadDatagram> {
    Some(ThreadDatagram {
        source: report.positional(0)?.parse().ok()?,
        port: report.positional(1)?.parse().ok()?,
        payload: hex(report.positional(2)?)?,
    })
}

/// What this driver expects of the peer's `@READY`: the OpenThread image,
/// not the IEEE 802.15.4 peer that shares the board.
const EXPECTED: Expected = Expected {
    peer: "Thread peer",
    protocol: THREAD_PEER_PROTOCOL,
    stack: Some("openthread"),
    restore: THREAD_PEER_REFLASH,
};

/// The Thread reference peer.
pub struct ThreadPeer<L> {
    console: PeerConsole<L>,
}

impl ThreadPeer<RecordingLink<SerialLink>> {
    /// Take over the peer on `path` and wait until it reports ready,
    /// recording every line of the session into `transcript`.
    pub fn open_recorded(path: &Path, transcript: &PeerTranscript) -> Result<Self> {
        Ok(Self {
            console: PeerConsole::open_recorded(
                oer_hil_lab::peer_console(path)?,
                transcript,
                EXPECTED,
                READY_TIMEOUT,
            )?,
        })
    }
}

impl<L: PeerLink> ThreadPeer<L> {
    /// Leave any network and close the socket with `SYNC`, and wait for the
    /// peer's `@READY` line on `link`.
    pub fn synchronize(link: L) -> Result<Self> {
        Ok(Self {
            console: PeerConsole::synchronize(link, EXPECTED, READY_TIMEOUT)?,
        })
    }

    /// Send `line` and wait for the answer to `name`; returns the reports
    /// named in `answer` printed before it.
    fn command(&mut self, name: &str, line: &str, answer: &[&str]) -> Result<Vec<Report>> {
        self.console.command(name, line, COMMAND_TIMEOUT, answer)
    }

    /// Form a new network on `channel` and `pan_id` as its leader.
    pub fn form(&mut self, channel: u8, pan_id: u16) -> Result<()> {
        self.command("FORM", &format!("FORM {channel} {pan_id:#06x}"), &[])
            .map(drop)
    }

    /// The active operational dataset TLVs.
    pub fn dataset(&mut self) -> Result<Vec<u8>> {
        self.command("DATASET", "DATASET", &["DATASET"])?
            .iter()
            .find_map(|report| hex(report.positional(0)?))
            .ok_or_else(|| "Thread peer answered DATASET without the dataset".into())
    }

    pub fn state(&mut self) -> Result<ThreadPeerState> {
        self.command("STATE", "STATE", &["STATE"])?
            .iter()
            .find_map(state)
            .ok_or_else(|| "Thread peer answered STATE without its state".into())
    }

    pub fn open_udp(&mut self, port: u16) -> Result<()> {
        self.command("UDP", &format!("UDP OPEN {port}"), &[])
            .map(drop)
    }

    pub fn send_udp(&mut self, destination: Ipv6Addr, port: u16, payload: &[u8]) -> Result<()> {
        self.command(
            "UDP",
            &format!("UDP SEND {destination} {port} {}", to_hex(payload)),
            &[],
        )
        .map(drop)
    }

    /// The next datagram the peer received before `timeout`, or `None`. A
    /// malformed report is skipped like any other line.
    pub fn next_datagram(&mut self, timeout: Duration) -> Result<Option<ThreadDatagram>> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let Some(report) = self.console.next_report(remaining, &["UDPRX"])? else {
                return Ok(None);
            };
            if let Some(datagram) = datagram(&report) {
                return Ok(Some(datagram));
            }
        }
    }

    /// The line transport, for the driver's tests.
    #[cfg(test)]
    pub(crate) fn link(&self) -> &L {
        self.console.link()
    }
}

#[cfg(test)]
mod tests;
