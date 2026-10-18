use std::{collections::VecDeque, time::Instant};

use super::*;

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
fn only_the_openthread_peer_is_ready() {
    // The IEEE 802.15.4 peer's ready line is another image.
    assert!(
        ThreadPeer::synchronize(ScriptedLink::new(&["@READY protocol=1 target=chip-b"])).is_err()
    );
    assert!(
        ThreadPeer::synchronize(ScriptedLink::new(&[
            "@READY protocol=2 target=chip-b stack=openthread"
        ]))
        .is_err()
    );
    ThreadPeer::synchronize(ScriptedLink::new(&[
        "boot noise",
        "@READY protocol=1 target=chip-b stack=openthread",
    ]))
    .unwrap();
}

#[test]
fn commands_render_and_return_their_reports() {
    let link = ScriptedLink::new(&[
        "@READY protocol=1 target=chip-b stack=openthread",
        "@OK FORM",
        "@DATASET 0e080000000000010000",
        "@OK DATASET",
        "@UDPRX fd00::2 1212 6869",
        "@STATE role=leader rloc16=fc00 eid=fd11:22::1",
        "@OK STATE",
        "@OK UDP",
        "@OK UDP",
    ]);
    let mut peer = ThreadPeer::synchronize(link).unwrap();
    peer.form(15, 0x4f45).unwrap();
    assert_eq!(
        peer.dataset().unwrap(),
        [0x0e, 0x08, 0, 0, 0, 0, 0, 1, 0, 0]
    );
    assert_eq!(
        peer.state().unwrap(),
        ThreadPeerState {
            role: "leader".into(),
            rloc16: 0xfc00,
            eid: "fd11:22::1".parse().unwrap(),
        }
    );
    peer.open_udp(1212).unwrap();
    peer.send_udp("fd11:22::2".parse().unwrap(), 1212, b"hi")
        .unwrap();
    assert_eq!(
        peer.link().sent,
        [
            "",
            "SYNC",
            "FORM 15 0x4f45",
            "DATASET",
            "STATE",
            "UDP OPEN 1212",
            "UDP SEND fd11:22::2 1212 6869",
        ]
    );
    // The datagram that arrived during STATE was kept.
    assert_eq!(
        peer.next_datagram(Duration::ZERO).unwrap(),
        Some(ThreadDatagram {
            source: "fd00::2".parse().unwrap(),
            port: 1212,
            payload: b"hi".to_vec(),
        })
    );
    assert_eq!(peer.next_datagram(Duration::ZERO).unwrap(), None);
}

#[test]
fn a_rejected_or_incomplete_answer_fails() {
    let link = ScriptedLink::new(&[
        "@READY protocol=1 target=chip-b stack=openthread",
        "@ERR FORM InvalidArgs",
        "@OK DATASET",
    ]);
    let mut peer = ThreadPeer::synchronize(link).unwrap();
    assert!(peer.form(15, 0x4f45).is_err());
    assert!(peer.dataset().is_err());
    assert!(peer.state().is_err());
}
