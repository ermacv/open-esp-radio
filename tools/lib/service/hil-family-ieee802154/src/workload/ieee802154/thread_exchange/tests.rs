use oer_hil_protocol::ieee802154::Ieee802154ThreadDatagram;

use super::*;

const DEVICE: Ipv6Addr = Ipv6Addr::new(0xfd00, 0, 0, 0, 0x1111, 0, 0, 1);
const PEER: Ipv6Addr = Ipv6Addr::new(0xfd00, 0, 0, 0, 0x2222, 0, 0, 2);

fn datagram(source: Ipv6Addr, port: u16, payload: &[u8]) -> ThreadDatagram {
    ThreadDatagram {
        source,
        port,
        payload: payload.to_vec(),
    }
}

#[test]
fn the_peer_must_receive_the_exact_datagram_from_the_device() {
    let sent = payload("D2P", 0);
    check_datagram(Some(datagram(DEVICE, UDP_PORT, &sent)), DEVICE, &sent).unwrap();
    for received in [
        None,
        Some(datagram(PEER, UDP_PORT, &sent)),
        Some(datagram(DEVICE, UDP_PORT + 1, &sent)),
        Some(datagram(DEVICE, UDP_PORT, &payload("D2P", 1))),
    ] {
        assert!(check_datagram(received, DEVICE, &sent).is_err());
    }
}

fn evidence(datagrams: &[(Ipv6Addr, &[u8])], total: u16) -> Ieee802154ThreadReceiveEvidence {
    Ieee802154ThreadReceiveEvidence {
        result: Ieee802154SessionResult::Done,
        total,
        datagrams: datagrams
            .iter()
            .map(|(source, payload)| Ieee802154ThreadDatagram {
                source: source.octets(),
                port: UDP_PORT,
                payload: Ieee802154ThreadPayload::from_slice(payload).unwrap(),
            })
            .collect(),
    }
}

#[test]
fn the_device_must_receive_every_datagram_in_order_from_the_peer() {
    let expected = [payload("P2D", 0), payload("P2D", 1)];
    let (first, second) = (expected[0].as_slice(), expected[1].as_slice());
    check_device_received(
        &evidence(&[(PEER, first), (PEER, second)], 2),
        PEER,
        &expected,
    )
    .unwrap();
    for wrong in [
        evidence(&[(PEER, first)], 1),
        evidence(&[(PEER, second), (PEER, first)], 2),
        evidence(&[(DEVICE, first), (PEER, second)], 2),
        // A duplicate beyond the recorded ones is still one too many.
        evidence(&[(PEER, first), (PEER, second)], 3),
    ] {
        assert!(check_device_received(&wrong, PEER, &expected).is_err());
    }
    let mut lost = evidence(&[(PEER, first), (PEER, second)], 2);
    lost.result = Ieee802154SessionResult::EventsLost;
    assert!(check_device_received(&lost, PEER, &expected).is_err());
}

#[test]
fn payloads_name_their_direction_and_index_and_fit_the_session() {
    assert_eq!(payload("D2P", 2), b"OER-THREAD-D2P-2");
    assert!(
        payload("P2D", u8::MAX).len()
            <= oer_hil_protocol::ieee802154::IEEE802154_THREAD_PAYLOAD_CAPACITY
    );
}
