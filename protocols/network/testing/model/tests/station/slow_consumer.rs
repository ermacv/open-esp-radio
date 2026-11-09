//! A slow consumer of received datagrams, each limit on its own: an
//! application that keeps its lending socket's whole credit while the
//! network is served, and a network task that is late while the access
//! point keeps sending. The first fills the socket's credit, never the
//! port's lend limit, and loses only its own datagrams. The second fills
//! the port's lend limit: the station holds its ring back for at most
//! [`NETWORK_LATE`] from the episode's first stop, then walks it, so it
//! still hears its access point, and refuses what the port cannot lend,
//! counted as pressure on its handoff. Every loan comes back once the
//! consumer resumes.
//!
//! The model's port lends [`MODEL_RX_EVENTS`] buffers, the lending socket's
//! credit is half of them ([`LEND_CREDIT`]), and the station's queue to the
//! network is deeper. The model's ring here is unbounded: a ring the radio
//! no longer walks shows as frames left in it ([`ModelAir::pending_rx`]).
//! What a finite ring does behind a held loan is the backend's case
//! (`a_held_early_loan_stops_a_finite_ring_once_the_walker_comes_round`):
//! walking this queue does not show that a finite ring keeps working.
//!
//! [`ModelAir::pending_rx`]: oer_protocols_ieee80211_testing_model::ModelAir::pending_rx

use oer_portable_time::Duration;
use oer_protocols_ieee80211_runtime::datapath::NETWORK_LATE;
use oer_protocols_ieee80211_runtime::station::contract::{DisconnectReason, Notice};
use oer_protocols_ieee80211_testing_model::MODEL_RX_EVENTS;
use oer_protocols_network_runtime::Received;

use crate::simulation::{LEND_CREDIT, PORT, Setup, World, data_of, simulate};

/// The peer's port.
const PEER_PORT: u16 = 7000;

/// The `n`th datagram: its number, then filler.
fn datagram(n: u16) -> Vec<u8> {
    let mut payload = vec![(n % 251) as u8; 600];
    payload[..2].copy_from_slice(&n.to_be_bytes());
    payload
}

fn send<T>(world: &World<'_, T>, numbers: core::ops::Range<u16>) {
    for n in numbers {
        world.ap().wired.send_udp(PEER_PORT, PORT, &datagram(n));
    }
}

/// The peer pings the station and the reply comes back.
async fn echoed<T>(world: &World<'_, T>, identifier: u16) {
    let before = world.ap().wired.echo_replies.len();
    world.ap().wired.ping(identifier, 1, b"still served");
    world
        .until("the echo reply", Duration::from_secs(1), |world| {
            world.ap().wired.echo_replies.len() == before + 1
        })
        .await;
}

/// The application keeps its lending socket's whole credit and takes no
/// more, while the network is served and the peer goes on sending. The
/// socket's credit fills first: the network drops each datagram beyond it
/// as `NoCredit`, which returns its buffer, so the network never holds all
/// the port may lend (the account keeps the rest as its reserve) and the
/// station copies or drops nothing. The ring is walked to its end and the
/// network answers the peer's ping. Once the application gives its
/// datagrams back, the next bursts of its credit reach it whole, and every
/// loan comes home.
#[test]
fn an_application_keeping_its_credit_loses_only_its_own_datagrams() {
    struct Net;
    const OVER: u16 = 8;
    let setup = Setup {
        lending: true,
        ..Setup::default()
    };
    let report = simulate::<Net>(setup, |world| {
        Box::pin(async move {
            let credit = u16::try_from(LEND_CREDIT).unwrap();
            world.join().await;
            send(world, 0..credit);
            let mut kept = Vec::new();
            for n in 0..credit {
                let mut received = world.receive().await;
                assert!(matches!(received, Received::Lent(_)));
                assert_eq!(data_of(&mut received), datagram(n));
                kept.push(received);
            }
            send(world, credit..credit + OVER);
            world.sleep(Duration::from_millis(1)).await;
            echoed(world, 1).await;
            world
                .until(
                    "the datagrams over the credit are dropped",
                    Duration::from_secs(1),
                    |world| world.counts().no_credit == usize::from(OVER),
                )
                .await;
            assert_eq!(world.air.pending_rx(), 0, "the ring is walked to its end");
            let counts = world.counts();
            assert_eq!(
                counts.rx_queue_full + counts.rx_pressure,
                0,
                "the port's lend limit never held the ring back past its bound"
            );
            assert_eq!(counts.rx_frames_dropped, 0);
            for received in kept {
                world.release(received);
            }
            // The credit again bounds what is in flight to the
            // application: bursts of it all arrive.
            for burst in 0..2_u16 {
                let first = 100 + burst * credit;
                send(world, first..first + credit);
                for n in first..first + credit {
                    let mut received = world.receive().await;
                    assert_eq!(data_of(&mut received), datagram(n));
                    world.release(received);
                }
            }
            assert_eq!(world.counts().no_credit, usize::from(OVER));
            assert!(
                world.with_socket(|socket| socket.try_receive()).is_none(),
                "no datagram over the credit came late"
            );
        })
    });
    report.assert_stopped_with_every_owner();
}

/// The network's task is late, its future and owners kept, while the peer
/// sends a burst and then the access point deauthenticates the station.
/// The port's lend limit fills first: the station lends that many frames
/// and holds the rest back in its ring, the Deauthentication behind them.
/// Once the episode has lasted its bound, the station walks the ring: it
/// refuses the frames the port cannot lend for pressure on its handoff,
/// counted, their buffers back at once, and hears the Deauthentication
/// while the network is still late.
/// Once the network resumes, it takes what the station queued and every
/// loan and block comes home.
#[test]
fn a_late_network_leaves_the_station_hearing_its_access_point() {
    struct Net;
    const BURST: u16 = 16;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            world.join().await;
            world.suspend_network(true);
            send(world, 0..BURST);
            world.sleep(Duration::from_millis(1)).await;
            assert_ne!(world.air.pending_rx(), 0, "held back within the bound");
            assert_eq!(world.counts().rx_pressure, 0);
            world.ap().deauthenticate(2);
            world
                .until(
                    "the station hears the access point",
                    Duration::from_secs(1),
                    |world| world.heard.disconnections() == 1,
                )
                .await;
            assert!(world.heard.notices.borrow().iter().any(|(_, notice)| {
                *notice == Notice::Disconnected(DisconnectReason::Peer { reason_code: 2 })
            }));
            assert_eq!(world.air.pending_rx(), 0, "the ring is walked to its end");
            let counts = world.counts();
            assert_eq!(counts.rx_pressure, usize::from(BURST) - MODEL_RX_EVENTS);
            assert_eq!(counts.rx_queue_full, 0);
            assert_eq!(
                counts.no_credit, 0,
                "no socket is named for the handoff's drops"
            );
            world.suspend_network(false);
        })
    });
    report.assert_stopped_with_every_owner();
}

/// The network's task is late while the peer sends a burst: the station
/// lends up to the port's limit and, once the episode has lasted its bound,
/// refuses the rest for pressure, counted, and walks the ring to its end.
/// Once
/// the network resumes, the application receives exactly what the station
/// queued, in order, the received and the dropped add up to the burst, the
/// network answers a ping, and a second burst reaches the application
/// whole: the loans came back.
#[test]
fn a_late_network_that_resumes_gets_what_was_queued_and_the_counts_close() {
    struct Net;
    const BURST: u16 = 16;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            world.join().await;
            world.suspend_network(true);
            send(world, 0..BURST);
            world
                .until("the ring is walked", Duration::from_secs(1), |world| {
                    world.counts().rx_pressure != 0 && world.air.pending_rx() == 0
                })
                .await;
            world.sleep(NETWORK_LATE).await;
            assert_eq!(world.air.pending_rx(), 0, "the ring is walked to its end");
            world.suspend_network(false);
            let queued = MODEL_RX_EVENTS;
            for n in 0..u16::try_from(queued).unwrap() {
                let mut received = world.receive().await;
                assert_eq!(data_of(&mut received), datagram(n));
                world.release(received);
            }
            echoed(world, 2).await;
            assert!(world.with_socket(|socket| socket.try_receive()).is_none());
            let counts = world.counts();
            assert_eq!(queued + counts.rx_pressure, usize::from(BURST));
            assert_eq!(counts.rx_queue_full, 0);
            send(world, 100..100 + BURST);
            for n in 100..100 + BURST {
                let mut received = world.receive().await;
                assert_eq!(data_of(&mut received), datagram(n));
                world.release(received);
            }
            assert_eq!(
                world.counts().rx_pressure,
                counts.rx_pressure,
                "a served network loses none"
            );
        })
    });
    report.assert_stopped_with_every_owner();
}
