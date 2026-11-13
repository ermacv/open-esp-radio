//! The application's control of the lifecycle: correlated refusals and
//! terminals, one admission per request kind, the product's reconnect
//! policy, the published status and the cooperative stops.

use super::*;
use crate::station::{OperationEnd, ReconnectPolicy};

/// A request the station refuses comes back with its own correlation, and
/// each admitted request ends once.
#[test]
fn application_requests_return_correlated_refusals_and_unique_completions() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let scan = world.station_command(StaCommand::BackgroundScan).await;
                world.settle().await;
                assert!(world.heard.command_refusals.borrow().contains(&(
                    scan,
                    StaCommand::BackgroundScan,
                    StaCommandRefusal::NotConnected
                )));
                let mut seen = 0;
                let connect = world.station_command(StaCommand::Connect).await;
                world.join(&mut seen, beacon(false), false).await;
                world.connected().await;
                world.settle().await;
                let again = world.station_command(StaCommand::Connect).await;
                world.settle().await;
                assert!(matches!(
                    world.heard.terminals_of(connect).as_slice(),
                    [StaCommandOutcome::Connected { .. }]
                ));
                assert!(world.heard.command_refusals.borrow().contains(&(
                    again,
                    StaCommand::Connect,
                    StaCommandRefusal::AlreadyConnected
                )));
                assert!(world.heard.terminals_of(again).is_empty());
                assert!(world.heard.terminals_of(scan).is_empty());
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// One admission per request kind: a second Connect is not posted while the
/// first holds its permission; the first's terminal admits it, and the
/// connected station refuses it.
#[test]
fn a_second_connect_waits_for_the_first_terminal_and_is_then_refused() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                let mut control = world.control.borrow_mut().take().unwrap();
                let first = control
                    .connect(world.profile())
                    .await
                    .expect("the first connect");
                let second = {
                    let mut second = core::pin::pin!(control.connect(world.profile()));
                    for _ in 0..16 {
                        assert!(
                            second
                                .as_mut()
                                .poll(&mut Context::from_waker(Waker::noop()))
                                .is_pending()
                        );
                        yield_now().await;
                    }
                    world.join(&mut seen, beacon(false), false).await;
                    assert!(matches!(
                        first.await,
                        OperationEnd::Completed(StaCommandOutcome::Connected { .. })
                    ));
                    second.await.expect("admitted after the first terminal")
                };
                assert_eq!(
                    second.await,
                    OperationEnd::Refused(StaCommandRefusal::AlreadyConnected)
                );
                *world.control.borrow_mut() = Some(control);
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// The product's policy: the application's Connect names the network, the
/// policy reconnects to it after the access point's departure under a new
/// epoch, and the graceful stop leaves (its Deauthentication on an open
/// link: `air::an_open_station_s_leave_sends_its_deauthentication`).
#[test]
fn product_policy_connects_reconnects_after_peer_departure_and_gracefully_stops() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                world.with_control(|control| {
                    control.configure_reconnect(
                        ReconnectPolicy::every(Duration::from_secs(1)).unwrap(),
                    )
                });
                world.station_command(StaCommand::Connect).await;
                let mut seen = 0;
                let mut last_epoch = 0;
                for attempt in 0..2 {
                    world.heard.notices.borrow_mut().clear();
                    world.join(&mut seen, beacon(false), false).await;
                    world.connected().await;
                    assert!(world.status().link.epoch > last_epoch);
                    last_epoch = world.status().link.epoch;
                    if attempt == 0 {
                        let mut departure = std::vec![0; 26];
                        departure[0] = 0xc0;
                        departure[4..10].copy_from_slice(&LOCAL);
                        departure[10..16].copy_from_slice(&BSSID);
                        departure[16..22].copy_from_slice(&BSSID);
                        departure[24] = 7;
                        world.air.receive(&departure, meta());
                        world
                            .notice(StaNotice::Disconnected(StaDisconnectReason::Peer {
                                reason_code: 7,
                            }))
                            .await;
                    }
                }
                world.with_control(|control| {
                    control.stop_gracefully(oer_portable_time::Instant::from_micros(u64::MAX))
                });
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::LocalRequest))
                    .await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// The application reads the link's state: down before each connection, up
/// with a new epoch once connected, down again with that epoch after the
/// disconnection.
#[test]
fn the_control_publishes_the_link_and_a_distinct_epoch_on_each_connection() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                let mut previous = 0;
                for _ in 0..2 {
                    assert_eq!(
                        world.status().link.state,
                        oer_protocols_network_port::LinkState::Down
                    );
                    world.associate(&mut seen, false).await;
                    world.connected().await;
                    let connected = world.status();
                    assert_eq!(
                        connected.link.state,
                        oer_protocols_network_port::LinkState::Up
                    );
                    assert_ne!(connected.link.epoch, previous);
                    previous = connected.link.epoch;
                    world.heard.notices.borrow_mut().clear();
                    world.station_command(StaCommand::Disconnect).await;
                    world
                        .notice(StaNotice::Disconnected(StaDisconnectReason::LocalRequest))
                        .await;
                    assert_eq!(
                        world.status().link,
                        oer_protocols_network_port::LinkSnapshot {
                            state: oer_protocols_network_port::LinkState::Down,
                            epoch: previous
                        }
                    );
                }
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// The control's cooperative stop ends the lifecycle and retires the actor;
/// the application hears the last Disconnected notice and reads the link
/// down.
#[test]
fn the_controls_cooperative_stop_retires_the_actor_and_publishes_the_last_notice() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world.with_control(|control| {
                    control.stop(oer_portable_time::Instant::from_micros(u64::MAX))
                });
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::LocalRequest))
                    .await;
                assert_eq!(
                    world.status().link.state,
                    oer_protocols_network_port::LinkState::Down
                );
                assert!(world.status().stopped);
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// The product's policy across a restart, as the station image runs it:
/// the application's Connect names the network, a fault restarts the
/// station, and the next installation joins that network with no command
/// of the application's; a leave there is admitted and ends.
#[test]
fn a_restarted_station_reconnects_by_its_policy_and_its_leave_is_answered() {
    pair(
        || two_channels(open()),
        std::vec![
            script(|world| {
                Box::pin(async move {
                    world.with_control(|control| {
                        control.configure_reconnect(
                            ReconnectPolicy::every(Duration::from_secs(1)).unwrap(),
                        )
                    });
                    let mut seen = 0;
                    world.associate(&mut seen, false).await;
                    world.connected().await;
                    // The background scan's move is refused, and so is the
                    // return home: the arbiter fails the station.
                    world.air.refuse_enables(2);
                    world.station_command(StaCommand::BackgroundScan).await;
                })
            }),
            script(|world| {
                Box::pin(async move {
                    let mut seen = world.air.sent().len();
                    world.heard.notices.borrow_mut().clear();
                    // No command: the policy's Connect.
                    world.join(&mut seen, beacon(false), false).await;
                    world.connected().await;
                    assert_eq!(
                        world.status().link.state,
                        oer_protocols_network_port::LinkState::Up
                    );
                    let mut control = world.control.borrow_mut().take().unwrap();
                    let leave = control.disconnect().await.expect("the leave is admitted");
                    *world.control.borrow_mut() = Some(control);
                    world
                        .notice(StaNotice::Disconnected(StaDisconnectReason::LocalRequest))
                        .await;
                    assert!(
                        matches!(leave.await, OperationEnd::Completed(_)),
                        "the leave ends"
                    );
                    world.stop().await;
                })
            }),
        ],
        |ends, _| {
            assert_eq!(ends.len(), 2);
            assert!(matches!(
                ends[0].lifecycle.as_ref().unwrap().end,
                LifecycleEnd::Restart(_)
            ));
            assert!(stopped(&ends[1..]));
        },
    );
}
