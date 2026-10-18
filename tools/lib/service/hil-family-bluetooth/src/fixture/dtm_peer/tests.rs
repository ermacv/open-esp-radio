use std::{collections::VecDeque, time::Instant};

use super::*;

/// A peer that answers each sent line from a script.
struct Scripted {
    sent: Vec<String>,
    replies: VecDeque<Vec<&'static str>>,
    pending: VecDeque<String>,
}

impl Scripted {
    fn new(replies: Vec<Vec<&'static str>>) -> Self {
        Self {
            sent: Vec::new(),
            replies: replies.into(),
            pending: VecDeque::new(),
        }
    }
}

impl PeerLink for Scripted {
    fn send(&mut self, line: &str) -> Result<()> {
        self.sent.push(line.to_owned());
        if !line.is_empty()
            && let Some(lines) = self.replies.pop_front()
        {
            self.pending
                .extend(lines.into_iter().map(|line| format!("{line}\n")));
        }
        Ok(())
    }

    fn receive(&mut self, _deadline: Instant) -> Result<Option<String>> {
        Ok(self.pending.pop_front())
    }
}

fn peer(replies: Vec<Vec<&'static str>>) -> DtmPeer<Scripted> {
    let mut script = vec![vec!["boot noise", "@READY protocol=1 target=chip-b"]];
    script.extend(replies);
    DtmPeer::synchronize(Scripted::new(script)).unwrap()
}

#[test]
fn a_test_is_started_and_ended_with_its_packet_count() {
    let mut peer = peer(vec![vec!["@OK TX"], vec!["@END packets=418", "@OK END"]]);
    peer.transmit(DtmVersion::V2, 19, 37, 0, DtmTxPhy::Le2M)
        .unwrap();
    assert_eq!(peer.end().unwrap(), 418);
    assert_eq!(peer.link().sent[2..], ["TX 2 19 37 0 2M", "END"]);
}

#[test]
fn a_refusal_names_its_hci_status() {
    let mut peer = peer(vec![vec!["@ERR RX status=0x01"]]);
    let error = peer
        .receive(DtmVersion::V2, 0, DtmRxPhy::Coded)
        .unwrap_err()
        .to_string();
    assert!(error.contains("HCI status 0x01"), "{error}");
}

#[test]
fn arguments_outside_the_protocol_are_refused_before_sending() {
    let mut peer = peer(Vec::new());
    assert!(peer.receive(DtmVersion::V1, 0, DtmRxPhy::Le2M).is_err());
    assert!(peer.receive(DtmVersion::V2, 40, DtmRxPhy::Le1M).is_err());
    assert!(
        peer.transmit(DtmVersion::V1, 0, 38, 0, DtmTxPhy::Le1M)
            .is_err()
    );
    assert!(
        peer.transmit(DtmVersion::V2, 0, 10, 8, DtmTxPhy::Le1M)
            .is_err()
    );
    assert_eq!(peer.link().sent, ["", "SYNC"]);
}

#[test]
fn a_peer_of_another_protocol_is_refused() {
    let link = Scripted::new(vec![vec!["@READY protocol=2 target=chip-b"]]);
    assert!(DtmPeer::synchronize(link).is_err());
}

#[test]
fn the_catalog_image_is_the_peer_project() {
    assert_eq!(DTM_PEER_IMAGE.name, "ble-dtm-peer");
}
