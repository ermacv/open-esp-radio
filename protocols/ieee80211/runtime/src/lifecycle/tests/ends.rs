//! The lifecycle's ends: a refused start, a faulted port, an unconfirmed
//! stop, a stop before Message 4, a restart and the frames a stop returns.

use super::*;

/// The radio refuses its first start: the lifecycle ends with the refusal,
/// the actor retires and nothing goes on the air. A Connect the application
/// sent meanwhile is never taken.
#[test]
fn a_refused_first_start_ends_the_lifecycle_and_retires_the_actor_without_a_frame() {
    pair_with::<2>(
        open,
        |air| air.refuse_enable(),
        std::vec![script(|world| {
            Box::pin(async move {
                let connect = world.station_command(StaCommand::Connect).await;
                world.settle().await;
                assert!(world.heard.terminals_of(connect).is_empty());
            })
        })],
        |ends, air| {
            let completed = ends[0].lifecycle.as_ref().unwrap();
            assert!(
                matches!(completed.end, LifecycleEnd::StartRefused(_)),
                "{:?}",
                completed.end
            );
            assert!(completed.end.is_terminal());
            assert!(completed.retired.rejected.is_none());
            assert_eq!(ends[0].actor, Some(ActorEnd::Retired));
            assert!(air.sent().is_empty());
            assert_eq!(air.disable_calls(), 0, "the radio never ran");
        },
    );
}

/// The radio's first start fails after the MAC began it, its stop not
/// confirmed: the lifecycle's end goes on with the port's own stop, and the
/// actor retires only once the port confirmed it.
#[test]
fn an_unstopped_first_start_ends_through_the_ports_stop_before_the_actor_retires() {
    pair_with::<2>(
        open,
        |air| air.unstop_enable(),
        std::vec![script(|world| {
            Box::pin(async move {
                world.settle().await;
            })
        })],
        |ends, air| {
            let completed = ends[0].lifecycle.as_ref().unwrap();
            assert!(
                matches!(
                    completed.end,
                    LifecycleEnd::StartRefused(
                        oer_protocols_ieee80211_contracts_port::StartRefused::Unstopped { .. }
                    )
                ),
                "{:?}",
                completed.end
            );
            assert_eq!(ends[0].actor, Some(ActorEnd::Retired));
            assert_eq!(
                air.confirmed_stops(),
                1,
                "the port's stop confirmed before the retirement"
            );
        },
    );
}

/// An unstopped start whose stop never confirms: the lifecycle ends with
/// the unconfirmed stop and the actor does not retire over a radio that is
/// not stopped.
#[test]
fn an_unstopped_start_whose_stop_never_confirms_ends_unconfirmed_without_a_retirement() {
    pair_with::<2>(
        open,
        |air| {
            air.unstop_enable();
            air.hold_stop(true);
        },
        std::vec![script(|world| {
            Box::pin(async move {
                world.settle().await;
            })
        })],
        |ends, air| {
            let completed = ends[0].lifecycle.as_ref().unwrap();
            assert!(
                matches!(completed.end, LifecycleEnd::StopUnconfirmed(_)),
                "{:?}",
                completed.end
            );
            assert_ne!(ends[0].actor, Some(ActorEnd::Retired));
            assert_eq!(air.confirmed_stops(), 0);
        },
    );
}

/// The port fails while the station scans: the actor ends on the port's
/// fault, and the lifecycle ends with it, telling the application the
/// connect's one terminal and one Disconnected notice.
#[test]
fn a_port_failure_during_the_scan_ends_both_tasks_with_one_connect_terminal_and_one_notice() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                let connect = world.station_command(StaCommand::Connect).await;
                world.sent(&mut seen, PROBE_REQUEST).await;
                world.air.poison();
                let reason =
                    StaDisconnectReason::Arbiter(crate::station::contract::LeaveReason::PortFailed);
                world.notice(StaNotice::Disconnected(reason)).await;
                assert_eq!(
                    world.heard.terminals_of(connect),
                    [StaCommandOutcome::ConnectFailed(reason)]
                );
                assert_eq!(world.heard.disconnections(), 1);
            })
        })],
        |ends, _| {
            let completed = ends[0].lifecycle.as_ref().unwrap();
            assert!(
                matches!(
                    completed.end,
                    LifecycleEnd::Faulted(crate::datapath::ActorFault::Port(_))
                ),
                "{:?}",
                completed.end
            );
            assert!(completed.end.is_terminal());
            assert!(matches!(
                ends[0].actor,
                Some(ActorEnd::Faulted(crate::datapath::ActorFault::Port(_)))
            ));
        },
    );
}

/// The port fails while a data arena is on the air: both tasks end on the
/// fault without a stop, so the radio is never disabled and the arena's
/// frame stays where it was.
#[test]
fn a_port_fault_with_an_arena_on_the_air_ends_both_tasks_without_a_stop() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world.connected().await;
                world
                    .air
                    .hold_completion(WmmAccessCategory::BestEffort, true);
                assert!(world.send_msdu([2, 0, 0, 0, 0, 7]));
                world.sent(&mut seen, DATA).await;
                world.air.poison();
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::Arbiter(
                        crate::station::contract::LeaveReason::PortFailed,
                    )))
                    .await;
                world.take_back_memory();
                assert_eq!(world.memory_back.get(), 0, "the arena keeps its frame");
            })
        })],
        |ends, air| {
            let completed = ends[0].lifecycle.as_ref().unwrap();
            assert!(
                matches!(
                    completed.end,
                    LifecycleEnd::Faulted(crate::datapath::ActorFault::Port(_))
                ),
                "{:?}",
                completed.end
            );
            assert!(matches!(ends[0].actor, Some(ActorEnd::Faulted(_))));
            assert_eq!(air.disable_calls(), 0, "a fault stops nothing");
        },
    );
}

/// The port fails after the application's stop began: the radio's stop
/// proves nothing, and the lifecycle ends unconfirmed after one terminal of
/// the connect and one Disconnected notice.
#[test]
fn a_port_failing_during_the_stop_ends_the_lifecycle_unconfirmed() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world.connected().await;
                world.settle().await;
                world.air.poison();
                world.stop().await;
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::Arbiter(
                        crate::station::contract::LeaveReason::PortFailed,
                    )))
                    .await;
                assert_eq!(world.heard.disconnections(), 1);
                assert_eq!(world.heard.terminals.borrow().len(), 1, "the connect's");
            })
        })],
        |ends, _| {
            let completed = ends[0].lifecycle.as_ref().unwrap();
            match &completed.end {
                LifecycleEnd::StopUnconfirmed(observed) => {
                    assert!(observed.port.is_some(), "{observed:?}");
                }
                end => panic!("an unconfirmed stop: {end:?}"),
            }
            assert!(completed.end.is_terminal());
        },
    );
}

/// The application stops, or the port fails, while the group key's
/// installation waits at the port's barrier: the lifecycle sends neither
/// Message 4 nor a Deauthentication, and the connect ends once, failed.
fn held_group_install_ends(poisoned: bool) {
    pair(
        wpa2,
        std::vec![script(move |world| {
            Box::pin(async move {
                let mut seen = 0;
                let connect = world.station_command(StaCommand::Connect).await;
                world.join(&mut seen, beacon(true), true).await;
                let ptk = world.start_handshake(&mut seen).await;
                let before = world.air.sent().len();
                world.air.hold_group_commands(true);
                world.air.receive(&message3(&ptk), meta());
                world.settle().await;
                assert_eq!(world.air.group_writes(), 0);
                assert_eq!(world.air.sent().len(), before);
                let reason = if poisoned {
                    world.air.poison();
                    StaDisconnectReason::Arbiter(crate::station::contract::LeaveReason::PortFailed)
                } else {
                    world.stop().await;
                    StaDisconnectReason::LocalRequest
                };
                world.notice(StaNotice::Disconnected(reason)).await;
                assert_eq!(
                    world.air.sent().len(),
                    before,
                    "the end sends no Message 4 and no Deauthentication"
                );
                assert_eq!(
                    world.heard.terminals_of(connect),
                    [StaCommandOutcome::ConnectFailed(reason)]
                );
                assert_eq!(world.heard.disconnections(), 1);
            })
        })],
        move |ends, _| {
            let completed = ends[0].lifecycle.as_ref().unwrap();
            if poisoned {
                assert!(
                    matches!(completed.end, LifecycleEnd::Faulted(_)),
                    "{:?}",
                    completed.end
                );
            } else {
                assert!(stopped(ends), "{:?}", completed.end);
            }
            assert!(completed.retired.rejected.is_none());
        },
    );
}

#[test]
fn a_fault_with_a_held_group_install_sends_no_message4() {
    held_group_install_ends(true);
}

#[test]
fn a_stop_with_a_held_group_install_sends_no_message4() {
    held_group_install_ends(false);
}

/// A station whose radio cannot move restarts on the next installation,
/// which joins a protected network, handshakes and sends its data under
/// the new installation's key.
#[test]
fn a_restarted_station_handshakes_and_sends_protected_data_on_the_next_installation() {
    pair(
        || two_channels(wpa2()),
        std::vec![
            script(|world| {
                Box::pin(async move {
                    let mut seen = 0;
                    world.associate(&mut seen, true).await;
                    world.handshake(&mut seen).await;
                    // The background scan's move is refused, and so is the
                    // return home: the arbiter fails the station.
                    world.air.refuse_enables(2);
                    world.station_command(StaCommand::BackgroundScan).await;
                })
            }),
            script(|world| {
                Box::pin(async move {
                    let mut seen = world.air.sent().len();
                    world.associate(&mut seen, true).await;
                    world.handshake(&mut seen).await;
                    assert!(world.send_msdu([0x02, 0, 0, 0, 0, 0x07]));
                    let data = world.sent(&mut seen, DATA).await;
                    assert_eq!(data.plaintext[4..10], BSSID);
                    assert!(matches!(data.key, Some((_, true))), "{data:?}");
                    world.stop().await;
                })
            }),
        ],
        |ends, air| {
            assert_eq!(ends.len(), 2);
            assert!(matches!(
                ends[0].lifecycle.as_ref().unwrap().end,
                LifecycleEnd::Restart(StationFailure::RetuneRefused(_))
            ));
            assert!(stopped(&ends[1..]));
            assert_eq!(air.pending_rx(), 0);
        },
    );
}

/// Frames the port received before the stop are returned by it: the stop
/// confirms with every received unit back in the port.
#[test]
fn a_stop_returns_the_frames_received_before_it() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                world.settle().await;
                for _ in 0..3 {
                    world.air.receive(&beacon(false), meta());
                }
                world.stop().await;
                world.settle().await;
            })
        })],
        |ends, air| {
            assert!(stopped(ends));
            assert_eq!(air.pending_rx(), 0, "every received unit came back");
        },
    );
}
