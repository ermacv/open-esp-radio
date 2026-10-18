//! Driver of the Bluetooth LE Direct Test Mode reference peer.
//!
//! The peer runs the BLE DTM project of `hil/peers`: the vendor
//! Controller behind a line protocol on its console (see that README). Every
//! command is answered by `@OK` or `@ERR`; `END` first prints the Controller's
//! packet count. The grammar, `SYNC`, the transport and the transcript are
//! the stand's one peer console ([`oer_hil_link::peer`]).

use std::{path::Path, time::Duration};

use oer_hil_link::peer::{
    Expected, PeerConsole, PeerLink, PeerRejected, PeerTranscript, RecordingLink, SerialLink,
};
use oer_hil_scenario::PeerImage;

use crate::Result;

/// Protocol version the driver speaks.
pub const DTM_PEER_PROTOCOL: u32 = 1;
/// The catalog image of the BLE DTM peer project of `hil/peers`.
pub const DTM_PEER_IMAGE: PeerImage = PeerImage {
    name: "ble-dtm-peer",
    reflash: "restore it with `cargo hil firmware flash ble-dtm-peer --board <peer board>`",
};
/// File name of a boot's peer transcript in its artifact directory.
pub const DTM_PEER_TRANSCRIPT: &str = "dtm-peer-transcript.log";
const READY_TIMEOUT: Duration = Duration::from_secs(5);
/// HCI Reset and a test command each complete well within this.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

/// The HCI LE test command version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmVersion {
    /// LE Receiver/Transmitter Test v1: LE 1M only, at most 37 octets.
    V1,
    V2,
}

/// The PHY of a receiver test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmRxPhy {
    Le1M,
    Le2M,
    /// LE Coded, either coding.
    Coded,
}

/// The PHY of a transmitter test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmTxPhy {
    Le1M,
    Le2M,
    CodedS8,
    CodedS2,
}

/// Why the peer refused a command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DtmPeerRefusal {
    /// The peer rejected the arguments.
    Invalid,
    /// The Controller did not complete the command in time.
    Timeout,
    /// The Controller completed it with this HCI status.
    Status(u8),
    /// A reason this driver does not know.
    Other(String),
}

impl std::fmt::Display for DtmPeerRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid => f.write_str("invalid"),
            Self::Timeout => f.write_str("timeout"),
            Self::Status(status) => write!(f, "HCI status 0x{status:02x}"),
            Self::Other(reason) => f.write_str(reason),
        }
    }
}

impl DtmPeerRefusal {
    /// The refusal an `@ERR <command> <reason>` line gives.
    fn parse(reason: &str) -> Self {
        match reason {
            "invalid" => Self::Invalid,
            "timeout" => Self::Timeout,
            other => other
                .strip_prefix("status=0x")
                .and_then(|status| u8::from_str_radix(status, 16).ok())
                .map_or_else(|| Self::Other(other.to_owned()), Self::Status),
        }
    }
}

/// What this driver expects of the peer's `@READY`.
const EXPECTED: Expected = Expected {
    peer: "the Direct Test Mode peer",
    protocol: DTM_PEER_PROTOCOL,
    stack: None,
    restore: "is `ble-dtm-peer` on the peer board?",
};

/// The Direct Test Mode reference peer.
pub struct DtmPeer<L> {
    console: PeerConsole<L>,
}

impl DtmPeer<RecordingLink<SerialLink>> {
    /// Open the peer's console without resetting it, record every line into
    /// `transcript`, and take the running peer over with `SYNC`.
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

impl<L: PeerLink> DtmPeer<L> {
    /// Take over a running peer: `SYNC` resets its Controller and reports
    /// ready with the protocol this driver speaks.
    pub fn synchronize(link: L) -> Result<Self> {
        Ok(Self {
            console: PeerConsole::synchronize(link, EXPECTED, READY_TIMEOUT)?,
        })
    }

    /// Start LE Receiver Test `version` on RF channel `channel` (0–39).
    pub fn receive(&mut self, version: DtmVersion, channel: u8, phy: DtmRxPhy) -> Result<()> {
        check_channel(channel)?;
        if version == DtmVersion::V1 && phy != DtmRxPhy::Le1M {
            return Err("LE Receiver Test v1 runs only on LE 1M".into());
        }
        let phy = match phy {
            DtmRxPhy::Le1M => "1M",
            DtmRxPhy::Le2M => "2M",
            DtmRxPhy::Coded => "CODED",
        };
        self.command(
            "RX",
            &format!("RX {} {channel} {phy}", version_number(version)),
        )
        .map(drop)
    }

    /// Start LE Transmitter Test `version` of `length` octets with HCI
    /// payload pattern `pattern` (0–7).
    pub fn transmit(
        &mut self,
        version: DtmVersion,
        channel: u8,
        length: u8,
        pattern: u8,
        phy: DtmTxPhy,
    ) -> Result<()> {
        check_channel(channel)?;
        if pattern > 7 {
            return Err(format!("DTM payload pattern {pattern} is not 0–7").into());
        }
        if version == DtmVersion::V1 && (phy != DtmTxPhy::Le1M || length > 37) {
            return Err("LE Transmitter Test v1 runs only on LE 1M with at most 37 octets".into());
        }
        let phy = match phy {
            DtmTxPhy::Le1M => "1M",
            DtmTxPhy::Le2M => "2M",
            DtmTxPhy::CodedS8 => "S8",
            DtmTxPhy::CodedS2 => "S2",
        };
        self.command(
            "TX",
            &format!(
                "TX {} {channel} {length} {pattern} {phy}",
                version_number(version)
            ),
        )
        .map(drop)
    }

    /// LE Test End: the received packet count after a receiver test, the
    /// sent count after a transmitter test (this Controller's behaviour).
    pub fn end(&mut self) -> Result<u32> {
        self.command("END", "END")?
            .ok_or_else(|| "the Direct Test Mode peer ended its test without a count".into())
    }

    /// HCI Reset, which also ends a test without a count.
    pub fn reset(&mut self) -> Result<()> {
        self.command("RESET", "RESET").map(drop)
    }

    /// Send `line` and wait for the answer to `name`; returns the `@END`
    /// count printed before it.
    fn command(&mut self, name: &str, line: &str) -> Result<Option<u32>> {
        let reports = self
            .console
            .command(name, line, COMMAND_TIMEOUT, &["END"])
            .map_err(|error| match error.downcast::<PeerRejected>() {
                Ok(refused) => format!(
                    "the Direct Test Mode peer refused `{line}`: {}",
                    DtmPeerRefusal::parse(&refused.reason)
                )
                .into(),
                Err(error) => error,
            })?;
        Ok(reports
            .iter()
            .rev()
            .find_map(|report| report.parsed::<u32>("packets")))
    }

    /// The console, for tests of the driver.
    #[cfg(test)]
    fn link(&self) -> &L {
        self.console.link()
    }
}

fn version_number(version: DtmVersion) -> u8 {
    match version {
        DtmVersion::V1 => 1,
        DtmVersion::V2 => 2,
    }
}

fn check_channel(channel: u8) -> Result<()> {
    if channel > 39 {
        return Err(format!("DTM RF channel {channel} is not 0–39").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
