//! Driver of the IEEE 802.15.4 reference peer.
//!
//! The peer runs the IEEE 802.15.4 peer project of `hil/peers`: the vendor
//! driver behind a line protocol on its console (see that README). Commands
//! are answered by `@OK`/`@ERR`; driver reports arrive as events at any time
//! and are queued while a command waits for its answer. The grammar, `SYNC`
//! and the transport are the stand's one peer console
//! ([`oer_hil_link::peer`]); this driver renders the commands and types the
//! reports.

use std::{path::Path, time::Duration};

use crate::Result;
use oer_device_peer_line::{Report, hex, to_hex};
use oer_hil_link::peer::{
    Expected, PeerConsole, PeerLink, PeerTranscript, RecordingLink, SerialLink,
};

/// Protocol version the driver speaks.
pub const PEER_PROTOCOL: u32 = 1;
/// Board-journal image name of the IEEE 802.15.4 peer project of `hil/peers`.
pub const PEER_IMAGE: &str = "ieee802154-peer";
/// How to restore the peer firmware when another consumer replaced it.
pub const PEER_REFLASH: &str =
    "restore it with `cargo hil firmware flash ieee802154-peer --board <peer board>`";
/// File name of a boot's peer transcript in its artifact directory.
pub const PEER_TRANSCRIPT: &str = "peer-transcript.log";
const READY_TIMEOUT: Duration = Duration::from_secs(5);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// Radio settings of the peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerConfig {
    pub channel: u8,
    pub pan_id: u16,
    pub short_address: u16,
    /// Extended address in over-the-air (little-endian) byte order.
    pub extended_address: [u8; 8],
    pub promiscuous: bool,
    pub power_dbm: i8,
}

/// One frame the peer received: MAC bytes without PHR and FCS.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerFrame {
    pub bytes: Vec<u8>,
    pub rssi_dbm: i8,
    pub lqi: u8,
    /// The peer's automatic acknowledgement carried frame pending.
    pub pending: bool,
    pub channel: u8,
}

/// The acknowledgement of one peer transmission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerAck {
    pub bytes: Vec<u8>,
    pub pending: bool,
    pub rssi_dbm: i8,
    pub lqi: u8,
}

/// One driver report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerEvent {
    Received(PeerFrame),
    Transmitted {
        acknowledgement: Option<PeerAck>,
    },
    /// The vendor `esp_i_tx_error_t` code.
    TransmitFailed(i32),
    EnergyDetected(i8),
}

/// Transmissions of one burst, reported when it stops.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerBurst {
    pub sent: u32,
    pub failed: u32,
}

/// The driver report `report` types, if it is one.
pub(crate) fn event(report: &Report) -> Option<PeerEvent> {
    match report.name.as_str() {
        "RX" => Some(PeerEvent::Received(PeerFrame {
            bytes: hex(report.positional(0)?)?,
            rssi_dbm: report.parsed("rssi")?,
            lqi: report.parsed("lqi")?,
            pending: report.flag("pending")?,
            channel: report.parsed("ch")?,
        })),
        "TXDONE" => {
            let acknowledgement = match report.field("ack")? {
                "-" => None,
                bytes => Some(PeerAck {
                    bytes: hex(bytes)?,
                    pending: report.flag("pending")?,
                    rssi_dbm: report.parsed("rssi")?,
                    lqi: report.parsed("lqi")?,
                }),
            };
            Some(PeerEvent::Transmitted { acknowledgement })
        }
        "TXFAIL" => Some(PeerEvent::TransmitFailed(
            report.positional(0)?.parse().ok()?,
        )),
        "ED" => Some(PeerEvent::EnergyDetected(
            report.positional(0)?.parse().ok()?,
        )),
        _ => None,
    }
}

/// The reports that are driver events.
const EVENTS: &[&str] = &["RX", "TXDONE", "TXFAIL", "ED"];

/// What this driver expects of the peer's `@READY`.
const EXPECTED: Expected = Expected {
    peer: "IEEE 802.15.4 peer",
    protocol: PEER_PROTOCOL,
    stack: None,
    restore: PEER_REFLASH,
};

/// Render the `CFG` command.
pub(crate) fn cfg_line(config: &PeerConfig) -> String {
    format!(
        "CFG {} {:04x} {:04x} {} {} {}",
        config.channel,
        config.pan_id,
        config.short_address,
        to_hex(&config.extended_address),
        u8::from(config.promiscuous),
        config.power_dbm,
    )
}

/// The reference peer.
pub struct Peer<L> {
    console: PeerConsole<L>,
}

impl Peer<SerialLink> {
    /// Take over the peer on `path` and wait until it reports ready.
    pub fn open(path: &Path) -> Result<Self> {
        Self::synchronize(oer_hil_lab::peer_console(path)?)
    }
}

impl Peer<RecordingLink<SerialLink>> {
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

impl<L: PeerLink> Peer<L> {
    /// Return the running peer on `link` to its defaults with `SYNC` and
    /// wait for its `@READY` line.
    pub fn synchronize(link: L) -> Result<Self> {
        Ok(Self {
            console: PeerConsole::synchronize(link, EXPECTED, READY_TIMEOUT)?,
        })
    }

    fn command(&mut self, name: &str, line: &str) -> Result<()> {
        self.console
            .command(name, line, COMMAND_TIMEOUT, &[])
            .map(drop)
    }

    pub fn configure(&mut self, config: &PeerConfig) -> Result<()> {
        self.command("CFG", &cfg_line(config))
    }

    pub fn receive(&mut self) -> Result<()> {
        self.command("RX", "RX")
    }

    pub fn sleep(&mut self) -> Result<()> {
        self.command("SLEEP", "SLEEP")
    }

    /// Start one transmission of MAC bytes (without FCS). Its outcome
    /// arrives as a [`PeerEvent`].
    pub fn transmit(&mut self, cca: bool, frame: &[u8]) -> Result<()> {
        self.command("TX", &format!("TX {} {}", u8::from(cca), to_hex(frame)))
    }

    /// Start transmitting MAC bytes (without FCS) back to back without CCA
    /// until [`Self::stop_burst`]; the peer reports no event for them.
    pub fn burst(&mut self, frame: &[u8]) -> Result<()> {
        self.command("BURST", &format!("BURST {}", to_hex(frame)))
    }

    /// Stop the burst after its last transmission and return its counts.
    pub fn stop_burst(&mut self) -> Result<PeerBurst> {
        self.console
            .command("BURST", "BURST STOP", COMMAND_TIMEOUT, &["BURST"])?
            .iter()
            .rev()
            .find_map(|report| {
                Some(PeerBurst {
                    sent: report.parsed("sent")?,
                    failed: report.parsed("failed")?,
                })
            })
            .ok_or_else(|| "IEEE 802.15.4 peer stopped a burst without its report".into())
    }

    pub fn set_pending_mode(&mut self, mode: u8) -> Result<()> {
        self.command("PENDING", &format!("PENDING {mode}"))
    }

    pub fn add_pending_short(&mut self, short_address: u16) -> Result<()> {
        self.command("PENDING", &format!("PENDING ADD {short_address:04x}"))
    }

    pub fn clear_pending(&mut self) -> Result<()> {
        self.command("PENDING", "PENDING CLEAR")
    }

    /// The next driver report before `timeout`, or `None`. A malformed
    /// report is skipped like any other line.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Option<PeerEvent>> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let Some(report) = self.console.next_report(remaining, EVENTS)? else {
                return Ok(None);
            };
            if let Some(event) = event(&report) {
                return Ok(Some(event));
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
