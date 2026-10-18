//! Direct Test Mode between the board under test and the reference peer.
//!
//! The board under test runs LE Receiver/Transmitter Test v2 over the image's raw
//! HCI exchange; the peer runs the vendor Controller's tests through its line
//! protocol. Each leg starts the receiver first, lets the transmitter run for
//! one window and ends the transmitter before the receiver. Two silence
//! controls end a receiver whose counterpart stayed idle; they must count
//! nothing, so the positive legs cannot be explained by another emitter.
use super::hci;
use crate::{
    Result,
    fixture::dtm_peer::{DTM_PEER_TRANSCRIPT, DtmPeer, DtmRxPhy, DtmTxPhy, DtmVersion},
};
use oer_hil_link::SerialCapture;
use oer_hil_link::peer::{PeerLink, PeerTranscript};
use oer_hil_workload::context::Context;
use serde::Serialize;
use std::{path::Path, thread, time::Duration};

const LE_RECEIVER_TEST_V2: u16 = 0x2033;
const LE_TRANSMITTER_TEST_V2: u16 = 0x2034;
const LE_TEST_END: u16 = 0x201f;
/// RF channel 19, 2440 MHz, away from the advertising channels.
const CHANNEL: u8 = 19;
const PAYLOAD_BYTES: u8 = 37;
/// HCI payload pattern PRBS9.
const PRBS9: u8 = 0;
const WINDOW: Duration = Duration::from_millis(500);

/// A PHY of the board's receiver and the peer's matching transmission.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ReceiveLeg {
    #[serde(rename = "le-1m")]
    Le1M,
    #[serde(rename = "le-2m")]
    Le2M,
    CodedS8,
    CodedS2,
}

impl ReceiveLeg {
    const ALL: [Self; 4] = [Self::Le1M, Self::Le2M, Self::CodedS8, Self::CodedS2];

    /// HCI LE Receiver Test v2 PHY: 1M, 2M or Coded.
    const fn hci_phy(self) -> u8 {
        match self {
            Self::Le1M => 1,
            Self::Le2M => 2,
            Self::CodedS8 | Self::CodedS2 => 3,
        }
    }

    const fn peer_phy(self) -> DtmTxPhy {
        match self {
            Self::Le1M => DtmTxPhy::Le1M,
            Self::Le2M => DtmTxPhy::Le2M,
            Self::CodedS8 => DtmTxPhy::CodedS8,
            Self::CodedS2 => DtmTxPhy::CodedS2,
        }
    }
}

/// A PHY of the board's transmitter and the peer's receiver.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum TransmitLeg {
    #[serde(rename = "le-1m")]
    Le1M,
    #[serde(rename = "le-2m")]
    Le2M,
}

impl TransmitLeg {
    const ALL: [Self; 2] = [Self::Le1M, Self::Le2M];

    /// HCI LE Transmitter Test v2 PHY.
    const fn hci_phy(self) -> u8 {
        match self {
            Self::Le1M => 1,
            Self::Le2M => 2,
        }
    }

    const fn peer_phy(self) -> DtmRxPhy {
        match self {
            Self::Le1M => DtmRxPhy::Le1M,
            Self::Le2M => DtmRxPhy::Le2M,
        }
    }
}

#[derive(Serialize)]
struct Received {
    phy: ReceiveLeg,
    /// Packets the peer reports it sent.
    peer_sent: u32,
    esp_received: u16,
}

#[derive(Serialize)]
struct Sent {
    phy: TransmitLeg,
    peer_received: u32,
}

#[derive(Default, Serialize)]
struct Report {
    esp_silence: Option<u16>,
    peer_silence: Option<u32>,
    esp_receives: Vec<Received>,
    esp_transmits: Vec<Sent>,
}

pub fn run(minimum_packets: u16, output: &Path, context: &Context<'_>) -> Result<()> {
    let peer_config = context.lab.peer()?;
    let transcript = PeerTranscript::default();
    let mut report = Report::default();
    let result = context.with_capture(output, |capture| {
        hci::require(capture)?;
        hci::command(capture, hci::RESET, &[])?;
        let mut peer = DtmPeer::open_recorded(&peer_config.serial()?, &transcript)?;
        let exercised = exercise(capture, &mut peer, &mut report);
        // Neither side may keep a test running after a failed leg.
        let peer_reset = peer.reset();
        let esp_reset = hci::command(capture, hci::RESET, &[]).map(drop);
        exercised.and(peer_reset).and(esp_reset)
    });
    transcript.save(&output.join(DTM_PEER_TRANSCRIPT))?;
    let outcome = result.and_then(|()| validate(&report, minimum_packets));
    context.results.observe(
        "dtm-peer",
        &serde_json::json!({
            "schema": 1,
            "channel": CHANNEL,
            "payload_bytes": PAYLOAD_BYTES,
            "payload": "PRBS9",
            "window_millis": WINDOW.as_millis() as u64,
            "minimum_packets": minimum_packets,
            "passed": outcome.is_ok(),
            "error": outcome.as_ref().err().map(ToString::to_string),
            "report": report,
        }),
    );
    outcome
}

fn exercise<L: PeerLink>(
    capture: &SerialCapture,
    peer: &mut DtmPeer<L>,
    report: &mut Report,
) -> Result<()> {
    // The board's receiver while the peer stays idle.
    esp_receive(capture, 1)?;
    thread::sleep(WINDOW);
    report.esp_silence = Some(esp_end(capture)?);
    // The peer's receiver while the board stays idle.
    peer.receive(DtmVersion::V2, CHANNEL, DtmRxPhy::Le1M)?;
    thread::sleep(WINDOW);
    report.peer_silence = Some(peer.end()?);
    for phy in ReceiveLeg::ALL {
        esp_receive(capture, phy.hci_phy())?;
        peer.transmit(
            DtmVersion::V2,
            CHANNEL,
            PAYLOAD_BYTES,
            PRBS9,
            phy.peer_phy(),
        )?;
        thread::sleep(WINDOW);
        let peer_sent = peer.end()?;
        let esp_received = esp_end(capture)?;
        report.esp_receives.push(Received {
            phy,
            peer_sent,
            esp_received,
        });
    }
    for phy in TransmitLeg::ALL {
        peer.receive(DtmVersion::V2, CHANNEL, phy.peer_phy())?;
        hci::command(
            capture,
            LE_TRANSMITTER_TEST_V2,
            &[CHANNEL, PAYLOAD_BYTES, PRBS9, phy.hci_phy()],
        )?;
        thread::sleep(WINDOW);
        esp_end(capture)?;
        let peer_received = peer.end()?;
        report.esp_transmits.push(Sent { phy, peer_received });
    }
    Ok(())
}

/// Start LE Receiver Test v2 with the standard modulation index.
fn esp_receive(capture: &SerialCapture, phy: u8) -> Result<()> {
    hci::command(capture, LE_RECEIVER_TEST_V2, &[CHANNEL, phy, 0]).map(drop)
}

/// LE Test End: the received packet count.
fn esp_end(capture: &SerialCapture) -> Result<u16> {
    let parameters = hci::command(capture, LE_TEST_END, &[])?;
    let count = parameters
        .get(..2)
        .ok_or("LE Test End returned no packet count")?;
    Ok(u16::from_le_bytes([count[0], count[1]]))
}

fn validate(report: &Report, minimum: u16) -> Result<()> {
    if report.esp_silence != Some(0) {
        return Err(format!(
            "the board's receiver counted {:?} packets while the peer was idle",
            report.esp_silence
        )
        .into());
    }
    if report.peer_silence != Some(0) {
        return Err(format!(
            "the peer receiver counted {:?} packets while the board was idle",
            report.peer_silence
        )
        .into());
    }
    if report.esp_receives.len() != ReceiveLeg::ALL.len()
        || report.esp_transmits.len() != TransmitLeg::ALL.len()
    {
        return Err("not every Direct Test Mode leg completed".into());
    }
    for leg in &report.esp_receives {
        if leg.esp_received < minimum {
            return Err(format!(
                "the board received {} of the peer's {} {:?} packets, below {minimum}",
                leg.esp_received, leg.peer_sent, leg.phy
            )
            .into());
        }
    }
    for leg in &report.esp_transmits {
        if leg.peer_received < u32::from(minimum) {
            return Err(format!(
                "the peer received {} {:?} packets from the board, below {minimum}",
                leg.peer_received, leg.phy
            )
            .into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing() -> Report {
        Report {
            esp_silence: Some(0),
            peer_silence: Some(0),
            esp_receives: ReceiveLeg::ALL
                .into_iter()
                .map(|phy| Received {
                    phy,
                    peer_sent: 400,
                    esp_received: 390,
                })
                .collect(),
            esp_transmits: TransmitLeg::ALL
                .into_iter()
                .map(|phy| Sent {
                    phy,
                    peer_received: 380,
                })
                .collect(),
        }
    }

    #[test]
    fn every_leg_and_both_silence_controls_are_required() {
        assert!(validate(&passing(), 50).is_ok());
        let mut report = passing();
        report.esp_silence = Some(1);
        assert!(validate(&report, 50).is_err());
        let mut report = passing();
        report.peer_silence = None;
        assert!(validate(&report, 50).is_err());
        let mut report = passing();
        report.esp_receives.pop();
        assert!(validate(&report, 50).is_err());
        let mut report = passing();
        report.esp_transmits[1].peer_received = 49;
        assert!(validate(&report, 50).is_err());
        let mut report = passing();
        report.esp_receives[2].esp_received = 49;
        assert!(validate(&report, 50).is_err());
    }

    #[test]
    fn legs_report_their_phy_names() {
        assert_eq!(serde_json::json!(ReceiveLeg::Le2M), "le-2m");
        assert_eq!(serde_json::json!(ReceiveLeg::CodedS8), "coded-s8");
        assert_eq!(serde_json::json!(TransmitLeg::Le1M), "le-1m");
    }

    #[test]
    fn coded_receiver_legs_select_the_coded_phy_for_both_codings() {
        assert_eq!(ReceiveLeg::CodedS8.hci_phy(), 3);
        assert_eq!(ReceiveLeg::CodedS2.hci_phy(), 3);
        assert_eq!(ReceiveLeg::CodedS2.peer_phy(), DtmTxPhy::CodedS2);
    }
}
