use std::{collections::VecDeque, time::Instant};

use oer_device_peer_line::{Line, parse};

use super::*;

/// The driver event a console line is, if it is one.
fn parse_event(line: &str) -> Option<PeerEvent> {
    match parse(line)? {
        Line::Report(report) => event(&report),
        _ => None,
    }
}

/// Replays scripted peer output; every sent line is recorded.
struct ScriptedLink {
    output: VecDeque<String>,
    sent: Vec<String>,
}

impl ScriptedLink {
    fn new(output: &[&str]) -> Self {
        Self {
            output: output.iter().map(|line| format!("{line}\n")).collect(),
            sent: Vec::new(),
        }
    }
}

impl PeerLink for ScriptedLink {
    fn send(&mut self, line: &str) -> Result<()> {
        self.sent.push(line.to_owned());
        Ok(())
    }

    fn receive(&mut self, _deadline: Instant) -> Result<Option<String>> {
        Ok(self.output.pop_front())
    }
}

#[test]
fn reports_type_as_events_and_other_output_is_ignored() {
    assert_eq!(parse_event("ESP-ROM:chip-b\n"), None);
    assert_eq!(
        parse_event("@RX 41880045 rssi=-47 lqi=255 pending=1 ch=15"),
        Some(PeerEvent::Received(PeerFrame {
            bytes: vec![0x41, 0x88, 0x00, 0x45],
            rssi_dbm: -47,
            lqi: 255,
            pending: true,
            channel: 15,
        }))
    );
    assert_eq!(
        parse_event("@TXDONE ack=-"),
        Some(PeerEvent::Transmitted {
            acknowledgement: None
        })
    );
    assert_eq!(
        parse_event("@TXDONE ack=120007 pending=0 rssi=-40 lqi=200"),
        Some(PeerEvent::Transmitted {
            acknowledgement: Some(PeerAck {
                bytes: vec![0x12, 0x00, 0x07],
                pending: false,
                rssi_dbm: -40,
                lqi: 200,
            })
        })
    );
    assert_eq!(parse_event("@TXFAIL 3"), Some(PeerEvent::TransmitFailed(3)));
    assert_eq!(parse_event("@ED -88"), Some(PeerEvent::EnergyDetected(-88)));
    for malformed in [
        "@RX 4 rssi=0 lqi=0 pending=0 ch=11",
        "@RX 41 rssi=x lqi=0 pending=0 ch=11",
        "@TXDONE",
        "@UNKNOWN",
    ] {
        assert_eq!(parse_event(malformed), None, "{malformed}");
    }
}

#[test]
fn the_configuration_renders_the_documented_command() {
    let config = PeerConfig {
        channel: 15,
        pan_id: 0x4f45,
        short_address: 0x0002,
        extended_address: [1, 2, 3, 4, 5, 6, 7, 8],
        promiscuous: false,
        power_dbm: -3,
    };
    assert_eq!(cfg_line(&config), "CFG 15 4f45 0002 0102030405060708 0 -3");
}

#[test]
fn commands_wait_for_their_answer_and_keep_interleaved_events() {
    let link = ScriptedLink::new(&[
        "boot noise",
        "@READY protocol=1 target=chip-b",
        "@RX 4188 rssi=-50 lqi=100 pending=0 ch=15",
        "@OK TX",
        "@TXDONE ack=-",
    ]);
    let mut peer = Peer::synchronize(link).unwrap();
    peer.transmit(false, &[0x41, 0x88]).unwrap();
    assert_eq!(peer.link().sent, ["", "SYNC", "TX 0 4188"]);
    assert!(matches!(
        peer.next_event(Duration::ZERO).unwrap(),
        Some(PeerEvent::Received(_))
    ));
    assert_eq!(
        peer.next_event(Duration::ZERO).unwrap(),
        Some(PeerEvent::Transmitted {
            acknowledgement: None
        })
    );
    assert_eq!(peer.next_event(Duration::ZERO).unwrap(), None);
}

#[test]
fn a_rejected_or_silent_command_fails() {
    let link = ScriptedLink::new(&["@READY protocol=1", "@ERR RX ESP_FAIL"]);
    let mut peer = Peer::synchronize(link).unwrap();
    assert!(peer.receive().is_err());
    assert!(peer.sleep().is_err());
}

#[test]
fn a_peer_of_another_protocol_is_refused() {
    assert!(Peer::synchronize(ScriptedLink::new(&["@READY protocol=2"])).is_err());
    assert!(Peer::synchronize(ScriptedLink::new(&["no ready line"])).is_err());
}

/// The recording link keeps every sent and received line, including the
/// synchronization, in order and with its direction.
#[test]
fn the_recording_link_keeps_the_whole_conversation() {
    let transcript = PeerTranscript::default();
    let link = RecordingLink::new(
        ScriptedLink::new(&["earlier output", "@READY protocol=1", "@OK SYNC", "@OK RX"]),
        transcript.clone(),
    );
    let mut peer = Peer::synchronize(link).unwrap();
    peer.receive().unwrap();
    // Each line starts with the milliseconds since the link opened.
    let lines: Vec<String> = transcript
        .lines()
        .iter()
        .map(|line| line.trim_start().split_once(' ').unwrap().1.to_owned())
        .collect();
    assert_eq!(
        lines,
        [
            "> ",
            "> SYNC",
            "< earlier output",
            "< @READY protocol=1",
            "> RX",
            "< @OK SYNC",
            "< @OK RX"
        ]
    );
}

#[test]
fn a_burst_stop_returns_the_report_printed_before_its_answer() {
    let link = ScriptedLink::new(&[
        "@READY protocol=1",
        "@OK BURST",
        "@BURST sent=412 failed=1",
        "@OK BURST",
        "@OK BURST",
    ]);
    let mut peer = Peer::synchronize(link).unwrap();
    peer.burst(&[0x41, 0x88]).unwrap();
    assert_eq!(
        peer.stop_burst().unwrap(),
        PeerBurst {
            sent: 412,
            failed: 1
        }
    );
    assert_eq!(peer.link().sent, ["", "SYNC", "BURST 4188", "BURST STOP"]);
    // A stop answered without its report is not a stopped burst.
    assert!(peer.stop_burst().is_err());
}

#[test]
fn a_mangled_sync_is_sent_again() {
    // The first bytes after the port opened were lost: the peer rejected
    // what reached it, then answered the repeated SYNC.
    let link = ScriptedLink::new(&["@ERR n unknown", "@READY protocol=1", "@OK SYNC"]);
    let peer = Peer::synchronize(link).unwrap();
    assert_eq!(peer.link().sent, ["", "SYNC", "", "SYNC"]);
    let silent = ScriptedLink::new(&["@ERR a unknown", "@ERR b unknown", "@ERR c unknown"]);
    assert!(Peer::synchronize(silent).is_err());
}
