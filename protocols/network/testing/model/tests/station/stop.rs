//! A graceful stop under load: the station leaves, the network ends every
//! command and brings every block home, and the station's owners return.
//! The network's transmit memory the station still holds at its end does
//! not all come back: the two ignored cases name that defect.

use oer_portable_time::Duration;
use oer_protocols_ieee80211_runtime::station::contract::{DisconnectReason, Notice};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;
use oer_protocols_network_core::{Outcome, Remote};
use oer_protocols_network_runtime::{Observer, Sent};

use crate::simulation::access_point::ApEvent;
use crate::simulation::wired::PEER_IP;
use crate::simulation::{NETWORK_BUFFERS, PORT, Report, Setup, TX_DEPTH, World, simulate};

const PEER: Remote = Remote {
    address: PEER_IP,
    port: 7000,
};

/// Join, and send one datagram so that the peer is resolved and what
/// follows goes straight on.
async fn joined_and_resolved<T>(world: &World<'_, T>) {
    world.join().await;
    let first = world.send(PEER, b"resolve").await;
    assert_eq!(world.result(first).await, Sent::Ended(Outcome::Accepted));
    world
        .until("the first datagram", Duration::from_secs(1), |world| {
            world.ap().wired.udp.len() == 1
        })
        .await;
}

/// Send `count` datagrams while the air holds every data completion: the
/// arena on the air never ends, the standby fills and the rest wait in the
/// station's queue.
async fn held_burst<T>(world: &World<'_, T>, count: u16) -> Vec<Observer<'static>> {
    world
        .air
        .hold_completion(WmmAccessCategory::BestEffort, true);
    let mut observers = Vec::new();
    for n in 0..count {
        observers.push(world.send(PEER, &[n as u8; 1000]).await);
    }
    observers
}

/// Stop, wait for the station's end and read every command's one result.
async fn stop_and_read<T>(world: &World<'_, T>, observers: Vec<Observer<'static>>) {
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
}

fn left_the_network(report: &Report) {
    let notices = report.heard.notices.borrow();
    assert_eq!(
        notices.last().map(|(_, notice)| *notice),
        Some(Notice::Disconnected(DisconnectReason::LocalRequest))
    );
    let ap = report.access_point.borrow();
    assert_eq!(
        ap.events.last().map(|(_, event)| event.clone()),
        Some(ApEvent::Left(3)),
        "the stop's Deauthentication, leaving"
    );
}

/// The application stops the station while an aggregate of its datagrams
/// is on the air with no end, more wait in the station and the network,
/// the peer's burst is arriving, and the application holds a received
/// datagram: the stop leaves the network, the lifecycle stops and the
/// actor retires, every command ends accepted or retired, and every block
/// comes home, the held datagram's through the pool's holder.
#[test]
fn a_stop_under_load_ends_every_command_and_brings_every_block_home() {
    struct Net;
    const BURST: u16 = 32;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            joined_and_resolved(world).await;
            let observers = held_burst(world, 24).await;
            for n in 0..BURST {
                world.ap().wired.send_udp(7000, PORT, &[n as u8; 800]);
            }
            let held = world.receive().await;
            world.hold(held);
            assert!(world.memory_free() < NETWORK_BUFFERS);
            stop_and_read(world, observers).await;
        })
    });
    report.assert_stopped();
    left_the_network(&report);
}

/// DEFECT: MSDUs the network handed to the station's transmit queue and the
/// actor never took stay in that queue after a graceful stop: the actor
/// retires without giving their memory back (`NetworkEnds::returned`), so
/// the network's transmit memory does not all come home.
#[test]
#[ignore = "defect: a stop leaves the network's MSDUs in the station's transmit queue"]
fn a_stop_gives_back_the_memory_of_msdus_still_in_the_station_s_queue() {
    struct Net;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            joined_and_resolved(world).await;
            let observers = held_burst(world, 24).await;
            stop_and_read(world, observers).await;
        })
    });
    report.assert_stopped();
    assert_eq!(report.memory_queued, 0, "MSDUs left in the station's queue");
    report.assert_transmit_memory_home();
}

/// DEFECT: the network takes ended frames' memory back from the station
/// only when it sends (`DeviceTransmit::has_room`, `poll_room`); while it
/// is idle the return queue fills at `TX_DEPTH` and the actor keeps the
/// rest in its own deque, which a graceful stop retires with the actor, so
/// the network's transmit memory does not all come home.
#[test]
#[ignore = "defect: a stop retires the actor holding ended frames' memory beyond the return queue"]
fn a_stop_after_an_idle_network_gives_back_every_ended_frame_s_memory() {
    struct Net;
    const BURST: u16 = 24;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            joined_and_resolved(world).await;
            let observers = held_burst(world, BURST).await;
            // The network has handed everything over and goes idle; the air
            // then ends every publication and the station sends the rest.
            world
                .air
                .hold_completion(WmmAccessCategory::BestEffort, false);
            world
                .until("the peer has the burst", Duration::from_secs(1), |world| {
                    world.ap().wired.udp.len() == 1 + usize::from(BURST)
                })
                .await;
            assert!(usize::from(BURST) > TX_DEPTH);
            stop_and_read(world, observers).await;
        })
    });
    report.assert_stopped();
    assert_eq!(report.memory_queued, 0);
    report.assert_transmit_memory_home();
}
