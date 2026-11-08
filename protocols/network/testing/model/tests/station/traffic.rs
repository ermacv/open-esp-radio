//! UDP through the application's socket, both ways, in bursts.

use std::collections::VecDeque;

use oer_portable_time::Duration;
use oer_protocols_network_core::{Outcome, Remote};
use oer_protocols_network_runtime::Sent;

use crate::simulation::wired::{LEASED_IP, PEER_IP};
use crate::simulation::{PORT, Setup, simulate};

/// The peer's port.
const PEER_PORT: u16 = 7000;

/// The `n`th datagram of a burst: its number, then filler.
fn datagram(n: u16, length: usize) -> Vec<u8> {
    let mut payload = vec![(n % 251) as u8; length];
    payload[..2].copy_from_slice(&n.to_be_bytes());
    payload
}

/// The peer sends bursts of datagrams to the socket's port, each burst in
/// one instant: every datagram reaches the application, whole and in
/// order, and the station drops none on its way.
#[test]
fn a_burst_from_the_peer_reaches_the_socket_whole_and_in_order() {
    struct Net;
    const BURSTS: u16 = 2;
    const BURST: u16 = 64;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            world.join().await;
            let mut next = 0_u16;
            for _ in 0..BURSTS {
                for n in next..next + BURST {
                    world
                        .ap()
                        .wired
                        .send_udp(PEER_PORT, PORT, &datagram(n, 1000));
                }
                for _ in 0..BURST {
                    let received = world.receive().await;
                    assert_eq!(
                        received.remote,
                        Remote {
                            address: PEER_IP,
                            port: PEER_PORT,
                        }
                    );
                    assert_eq!(received.block.octets(), datagram(next, 1000));
                    next += 1;
                    world.release(received);
                }
            }
            assert_eq!(next, BURSTS * BURST);
            assert!(
                world.with_socket(|socket| socket.try_receive()).is_none(),
                "no datagram more than the peer sent"
            );
            assert_eq!(world.counts().rx_frames_dropped, 0);
            assert_eq!(world.ap().dropped_to_station, 0);
        })
    });
    report.assert_stopped_with_every_owner();
}

/// The application sends a burst of datagrams to the peer, keeping up to
/// half its credits' results open: each ends accepted, and the peer
/// receives every one, whole and in order, from the leased address.
#[test]
fn a_burst_from_the_socket_reaches_the_peer_whole_and_in_order() {
    struct Net;
    const BURST: u16 = 96;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            world.join().await;
            let peer = Remote {
                address: PEER_IP,
                port: PEER_PORT,
            };
            let mut open = VecDeque::new();
            for n in 0..BURST {
                open.push_back(world.send(peer, &datagram(n, 1200)).await);
                if open.len() == crate::simulation::CREDITS / 2 {
                    let sent = world.result(open.pop_front().expect("a result")).await;
                    assert_eq!(sent, Sent::Ended(Outcome::Accepted));
                }
            }
            while let Some(observer) = open.pop_front() {
                assert_eq!(world.result(observer).await, Sent::Ended(Outcome::Accepted));
            }
            world
                .until("the peer has the burst", Duration::from_secs(1), |world| {
                    world.ap().wired.udp.len() == usize::from(BURST)
                })
                .await;
            let ap = world.ap();
            for (n, received) in ap.wired.udp.iter().enumerate() {
                assert_eq!(received.source, (LEASED_IP, PORT));
                assert_eq!(received.port, PEER_PORT);
                assert_eq!(received.payload, datagram(n as u16, 1200));
            }
        })
    });
    report.assert_stopped_with_every_owner();
    assert_eq!(
        report.access_point.borrow().wired.udp.len(),
        usize::from(BURST),
        "no datagram twice"
    );
}
