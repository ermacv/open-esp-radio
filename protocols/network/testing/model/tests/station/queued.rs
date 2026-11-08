//! The product's queued egress over the station: the airtime egress with
//! the station's air as one account per association. Each frame the port
//! took settles its reservation once the station gave its buffer back,
//! with the cost its transmissions had; a full socket refuses before
//! anything moved; a stop keeps no reservation without its buffer out.

use oer_portable_time::Duration;
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;
use oer_protocols_network_core::{Outcome, Remote};
use oer_protocols_network_runtime::{Refusal, Sent};

use crate::simulation::wired::PEER_IP;
use crate::simulation::{BLOCKS, CREDITS, Setup, simulate};

const PEER: Remote = Remote {
    address: PEER_IP,
    port: 7000,
};

/// A burst goes out: every frame the port took, control frames among them,
/// settles with an estimated cost once its buffer came back, and none
/// stays reserved at the end.
#[test]
fn every_frame_the_port_took_settles_its_reservation_with_its_cost() {
    struct Net;
    const BURST: usize = 40;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            world.join().await;
            for n in 0..BURST {
                let sent = world.send(PEER, &[n as u8; 600]).await;
                assert_eq!(world.result(sent).await, Sent::Ended(Outcome::Accepted));
            }
            world
                .until("the peer has the burst", Duration::from_secs(1), |world| {
                    world.ap().wired.udp.len() == BURST
                })
                .await;
        })
    });
    report.assert_stopped_with_every_owner();
    let (known, unknown) = report.settled;
    assert!(known >= BURST, "every datagram settled: {known}");
    assert_eq!(unknown, 0, "the model's costs are all known");
    assert_eq!(report.in_flight, 0, "no reservation outlives its frame");
}

/// With every credit's result unread, the socket refuses the next
/// datagram at once and gives its block back; reading one result frees
/// that credit for it.
#[test]
fn a_full_socket_refuses_with_the_block_and_a_read_result_frees_its_credit() {
    #[derive(Debug)]
    struct Net;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            world.join().await;
            let mut open = Vec::new();
            for n in 0..CREDITS {
                open.push(world.send(PEER, &[n as u8; 100]).await);
            }
            let mut socket_block = None;
            world
                .until("a block", Duration::from_secs(1), |world| {
                    socket_block = world.with_socket(|socket| socket.try_reserve().unwrap());
                    socket_block.is_some()
                })
                .await;
            let mut block = socket_block.expect("a block");
            block.fill(b"one too many").unwrap();
            let Err(refused) = world.with_socket(|socket| socket.send(PEER, block)) else {
                panic!("no credit is free");
            };
            assert_eq!(refused.reason, Refusal::Busy);
            assert_eq!(
                refused.block.octets(),
                b"one too many",
                "the block, as it was"
            );
            let first = open.remove(0);
            assert_eq!(world.result(first).await, Sent::Ended(Outcome::Accepted));
            let Ok(sent) = world.with_socket(|socket| socket.send(PEER, refused.block)) else {
                panic!("the read result's credit is free");
            };
            open.push(sent);
            for observer in open {
                assert_eq!(world.result(observer).await, Sent::Ended(Outcome::Accepted));
            }
        })
    });
    report.assert_stopped_with_every_owner();
}

/// A stop while the air holds every data completion: datagrams wait in the
/// network, the port and the station. Every command ends accepted or
/// retired, every block comes home, and each reservation the egress still
/// holds has its frame's buffer out with the station: none without one.
#[test]
fn a_stop_with_queued_work_keeps_no_reservation_without_its_buffer_out() {
    struct Net;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            world.join().await;
            world
                .air
                .hold_completion(WmmAccessCategory::BestEffort, true);
            let mut observers = Vec::new();
            for n in 0..CREDITS {
                observers.push(world.send(PEER, &[n as u8; 1000]).await);
            }
            world.stop(Duration::from_secs(10));
            world
                .until("the station stops", Duration::from_secs(10), |world| {
                    world.stopped()
                })
                .await;
            for observer in observers {
                let sent = world.result(observer).await;
                assert!(
                    matches!(sent, Sent::Ended(Outcome::Accepted | Outcome::Retired)),
                    "{sent:?}"
                );
            }
        })
    });
    report.assert_stopped();
    assert_eq!(report.blocks_free, BLOCKS);
    let out = report.memory - report.memory_free;
    assert!(
        report.in_flight <= out,
        "{} reservations, {out} buffers out",
        report.in_flight
    );
}
