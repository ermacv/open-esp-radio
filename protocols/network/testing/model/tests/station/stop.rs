//! A graceful stop under load: the station leaves, the network ends every
//! command and brings every block home, the station's owners return, and
//! every transmit buffer of the network is free again.

use oer_portable_time::Duration;
use oer_protocols_ieee80211_runtime::datapath::ActorEnd;
use oer_protocols_ieee80211_runtime::station::contract::{DisconnectReason, Notice};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;
use oer_protocols_network_core::{Outcome, Remote};
use oer_protocols_network_runtime::{Observer, Sent};

use crate::simulation::access_point::ApEvent;
use crate::simulation::wired::PEER_IP;
use crate::simulation::{
    NETWORK_BUFFERS, PORT, Report, Setup, TX_DEPTH, World, session, simulate, simulate_sessions,
};

const PEER: Remote = Remote {
    address: PEER_IP,
    port: 7000,
};

/// Join, and send one datagram so that the peer is resolved and what
/// follows goes straight on.
async fn joined_and_resolved<T>(world: &World<'_, T>) {
    world.join().await;
    let before = world.ap().wired.udp.len();
    let first = world.send(PEER, b"resolve").await;
    assert_eq!(world.result(first).await, Sent::Ended(Outcome::Accepted));
    world
        .until("the first datagram", Duration::from_secs(1), |world| {
            world.ap().wired.udp.len() == before + 1
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
    report.assert_stopped_with_every_owner();
    left_the_network(&report);
}

/// As above, with a lending socket that holds the station's receive buffer
/// across the stop: the station's stop confirms without waiting for it, as
/// the lend count outlives the installation, and once the application drops
/// it after the stop, its loan comes home to the pool's holder.
#[test]
fn a_stop_with_a_lent_buffer_held_confirms_and_its_late_return_frees_the_loan() {
    struct Net;
    const BURST: u16 = 32;
    let setup = Setup {
        lending: true,
        ..Setup::default()
    };
    let report = simulate::<Net>(setup, |world| {
        Box::pin(async move {
            joined_and_resolved(world).await;
            let observers = held_burst(world, 24).await;
            for n in 0..BURST {
                world.ap().wired.send_udp(7000, PORT, &[n as u8; 800]);
            }
            let held = world.receive().await;
            assert!(
                matches!(held, oer_protocols_network_runtime::Received::Lent(_)),
                "a single MSDU stays in the station's buffer"
            );
            world.hold(held);
            stop_and_read(world, observers).await;
        })
    });
    report.assert_stopped_with_every_owner();
    left_the_network(&report);
}

/// MSDUs the network handed to the station's transmit queue and the actor
/// never took go back with the stop: the actor hands their memory back
/// before it asks the network to stop, and the network takes it back at its
/// end.
#[test]
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

/// Ended frames' memory comes home while the network is idle: the network
/// takes it back whenever the station returns some, not only when it sends,
/// so the actor holds none beyond the return queue, and a stop after an
/// idle stretch finds every buffer home.
#[test]
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

/// The owners a stop returned start the station and the network again: the
/// actor reinstalls the model's port, a station on the new installation's
/// tokens takes the retired station's configuration, the control opens a
/// new epoch and the network's core takes the retired pool and brand, the
/// same device port and transmit memory. The second session joins with a
/// second handshake, leases its address, carries an echo and a burst both
/// ways, and its own stop brings every owner home again.
#[test]
fn a_stop_under_load_returns_owners_that_start_the_station_again() {
    struct Net;
    const BURST: u16 = 32;
    let reports = simulate_sessions::<Net>(
        Setup::default(),
        vec![
            session(|world| {
                Box::pin(async move {
                    joined_and_resolved(world).await;
                    let observers = held_burst(world, 24).await;
                    for n in 0..BURST {
                        world.ap().wired.send_udp(7000, PORT, &[n as u8; 800]);
                    }
                    let held = world.receive().await;
                    world.hold(held);
                    stop_and_read(world, observers).await;
                    world
                        .air
                        .hold_completion(WmmAccessCategory::BestEffort, false);
                })
            }),
            session(|world| {
                Box::pin(async move {
                    assert!(!world.stopped(), "a new control epoch");
                    joined_and_resolved(world).await;
                    world.ap().wired.ping(9, 1, b"again");
                    world
                        .until("the echo reply", Duration::from_secs(1), |world| {
                            world.ap().wired.echo_replies.len() == 1
                        })
                        .await;
                    for n in 0..BURST {
                        world.ap().wired.send_udp(7000, PORT, &[n as u8; 800]);
                    }
                    for _ in 0..BURST {
                        let received = world.receive().await;
                        world.release(received);
                    }
                    let mut observers = Vec::new();
                    for n in 0..BURST {
                        observers.push(world.send(PEER, &[n as u8; 1000]).await);
                    }
                    for observer in observers {
                        assert_eq!(world.result(observer).await, Sent::Ended(Outcome::Accepted));
                    }
                })
            }),
        ],
    );
    assert_eq!(reports.len(), 2);
    for report in &reports {
        report.assert_stopped_with_every_owner();
        left_the_network(report);
    }
    let ap = reports[1].access_point.borrow();
    assert_eq!(ap.associations(), 2, "a second association");
    let authorized = ap
        .events
        .iter()
        .filter(|(_, event)| matches!(event, ApEvent::Authorized(_)))
        .count();
    assert_eq!(authorized, 2, "a second handshake");
}

/// The network's service is late with its return: the application holds
/// 24 accepted datagrams in the station, the network's task is suspended
/// with its future and owners kept, and the graceful stop names a 10 ms
/// deadline. The radio stops; at the deadline the stop's wait ends as
/// returning, not unconfirmed, and the station's control reports it
/// stopped, while the memory the network has not taken back stays the
/// station's. Once the same service resumes, the return completes: the
/// network stops, the actor retires, every block and transmit buffer is
/// home, and the owners start a second session that joins, leases, pings
/// and carries data both ways.
#[test]
fn a_network_that_resumes_after_the_deadline_completes_the_return() {
    use oer_protocols_ieee80211_runtime::lifecycle::LifecycleEnd;
    struct Net;
    let reports = simulate_sessions::<Net>(
        Setup::default(),
        vec![
            session(|world| {
                Box::pin(async move {
                    joined_and_resolved(world).await;
                    let observers = held_burst(world, 24).await;
                    for observer in observers {
                        assert_eq!(world.result(observer).await, Sent::Ended(Outcome::Accepted));
                    }
                    world.suspend_network(true);
                    world
                        .air
                        .hold_completion(WmmAccessCategory::BestEffort, false);
                    world.stop(Duration::from_millis(10));
                    world
                        .until("the stop's wait ends", Duration::from_secs(1), |world| {
                            world.stopped()
                        })
                        .await;
                    assert!(
                        world.memory_free() < NETWORK_BUFFERS,
                        "the memory is not free before the network took it back"
                    );
                    world.sleep(Duration::from_millis(100)).await;
                    assert!(world.memory_free() < NETWORK_BUFFERS);
                    world.suspend_network(false);
                })
            }),
            session(|world| {
                Box::pin(async move {
                    joined_and_resolved(world).await;
                    world.ap().wired.ping(5, 1, b"after a late return");
                    world
                        .until("the echo reply", Duration::from_secs(1), |world| {
                            world.ap().wired.echo_replies.len() == 1
                        })
                        .await;
                    for n in 0..16_u16 {
                        world.ap().wired.send_udp(7000, PORT, &[n as u8; 600]);
                    }
                    for _ in 0..16 {
                        let received = world.receive().await;
                        world.release(received);
                    }
                    let mut observers = Vec::new();
                    for n in 0..16_u16 {
                        observers.push(world.send(PEER, &[n as u8; 900]).await);
                    }
                    for observer in observers {
                        assert_eq!(world.result(observer).await, Sent::Ended(Outcome::Accepted));
                    }
                })
            }),
        ],
    );
    let late = &reports[0];
    match late.lifecycle {
        Some(LifecycleEnd::StoppedReturning(observed)) => {
            assert!(observed.memory_held > 0, "{observed:?}");
            assert_eq!(observed.lent, 0);
        }
        ref end => panic!("a late network return ends the wait returning: {end:?}"),
    }
    late.assert_owners_home();
    late.assert_transmit_memory_home();
    left_the_network(late);
    reports[1].assert_stopped_with_every_owner();
    left_the_network(&reports[1]);
}

/// The network's stop participant is lost, once before and once after the
/// stop's deadline: each time it goes unreported while the network's owners
/// stay with the composition, which keeps serving their returns; before the
/// deadline the network's run is also dropped mid-traffic. The
/// radio's stop is confirmed and the loss is a handled, non-terminal end
/// the composition records: before the deadline the stop ends with the
/// loss, after it the late return completes with it. Every block, buffer
/// and lent unit comes home each time, and a third session starts again
/// from the same owners.
#[test]
fn a_lost_network_participant_before_and_after_the_deadline_is_handled_and_the_owners_reused() {
    use oer_protocols_ieee80211_runtime::lifecycle::LifecycleEnd;
    struct Net;
    let reports = simulate_sessions::<Net>(
        Setup::default(),
        vec![
            // Lost before the deadline: the network's run is dropped
            // mid-traffic and its participant goes unreported, while the
            // composition, which owns the service, serves the returns.
            session(|world| {
                Box::pin(async move {
                    joined_and_resolved(world).await;
                    let observers = held_burst(world, 24).await;
                    for n in 0..8_u16 {
                        world.ap().wired.send_udp(7000, PORT, &[n as u8; 400]);
                    }
                    world.lose_network_stop();
                    world.cancel_network_run();
                    world
                        .air
                        .hold_completion(WmmAccessCategory::BestEffort, false);
                    stop_and_read(world, observers).await;
                })
            }),
            // Lost after the deadline: the network is late, then ends
            // without reporting.
            session(|world| {
                Box::pin(async move {
                    joined_and_resolved(world).await;
                    let observers = held_burst(world, 24).await;
                    for observer in observers {
                        assert_eq!(world.result(observer).await, Sent::Ended(Outcome::Accepted));
                    }
                    world.suspend_network(true);
                    world
                        .air
                        .hold_completion(WmmAccessCategory::BestEffort, false);
                    world.lose_network_stop();
                    world.stop(Duration::from_millis(10));
                    world
                        .until("the stop's wait ends", Duration::from_secs(1), |world| {
                            world.stopped()
                        })
                        .await;
                    world.sleep(Duration::from_millis(100)).await;
                    world.suspend_network(false);
                })
            }),
            // The same owners again.
            session(|world| {
                Box::pin(async move {
                    joined_and_resolved(world).await;
                    world.ap().wired.ping(7, 1, b"after two losses");
                    world
                        .until("the echo reply", Duration::from_secs(1), |world| {
                            world.ap().wired.echo_replies.len() == 1
                        })
                        .await;
                    let mut observers = Vec::new();
                    for n in 0..16_u16 {
                        observers.push(world.send(PEER, &[n as u8; 900]).await);
                    }
                    for observer in observers {
                        assert_eq!(world.result(observer).await, Sent::Ended(Outcome::Accepted));
                    }
                })
            }),
        ],
    );
    let [before, after, again] = &reports[..] else {
        panic!("three sessions");
    };
    match &before.lifecycle {
        Some(end @ LifecycleEnd::StoppedNetworkLost(_)) => assert!(!end.is_terminal()),
        end => panic!("a loss before the deadline ends with the loss: {end:?}"),
    }
    match &after.lifecycle {
        Some(end @ LifecycleEnd::StoppedReturning(_)) => assert!(!end.is_terminal()),
        end => panic!("a late network ends the wait returning: {end:?}"),
    }
    for (report, lost) in [(before, true), (after, true), (again, false)] {
        assert_eq!(report.network_lost, lost, "the composition reads the loss");
        assert!(matches!(report.actor, Some(ActorEnd::Retired)));
        report.assert_owners_home();
        report.assert_transmit_memory_home();
        left_the_network(report);
    }
    again.assert_stopped_with_every_owner();
}

/// A host composition that cancels its network: the network service's real
/// `run` future is dropped mid-traffic, with an aggregate held on the air,
/// 24 datagrams in the station and a receive burst arriving, while the
/// composition, the owner that outlives that future, keeps the service. It
/// retires it and serves the port's returns from then on, so the station's
/// later graceful stop finds its memory taken back and its frames given
/// back, and confirms. Every block and transmit buffer is home, and a
/// second session starts from the same owners. This is the harness's
/// composition, not the S31 application's, which does not cancel its
/// network task.
#[test]
fn a_dropped_network_run_is_served_by_its_owner_and_every_owner_returns() {
    struct Net;
    let reports = simulate_sessions::<Net>(
        Setup::default(),
        vec![
            session(|world| {
                Box::pin(async move {
                    joined_and_resolved(world).await;
                    let observers = held_burst(world, 24).await;
                    for n in 0..16_u16 {
                        world.ap().wired.send_udp(7000, PORT, &[n as u8; 400]);
                    }
                    assert!(world.memory_free() < NETWORK_BUFFERS, "mid-traffic");
                    world.cancel_network_run();
                    world
                        .air
                        .hold_completion(WmmAccessCategory::BestEffort, false);
                    world.sleep(Duration::from_millis(50)).await;
                    assert_eq!(
                        world.memory_free(),
                        NETWORK_BUFFERS,
                        "the owner took back every ended frame's memory"
                    );
                    stop_and_read(world, observers).await;
                })
            }),
            session(|world| {
                Box::pin(async move {
                    joined_and_resolved(world).await;
                    for n in 0..16_u16 {
                        world.ap().wired.send_udp(7000, PORT, &[n as u8; 600]);
                    }
                    for _ in 0..16 {
                        let received = world.receive().await;
                        world.release(received);
                    }
                    let mut observers = Vec::new();
                    for n in 0..16_u16 {
                        observers.push(world.send(PEER, &[n as u8; 900]).await);
                    }
                    for observer in observers {
                        assert_eq!(world.result(observer).await, Sent::Ended(Outcome::Accepted));
                    }
                })
            }),
        ],
    );
    for report in &reports {
        report.assert_stopped_with_every_owner();
        assert!(!report.network_lost);
        left_the_network(report);
    }
}
