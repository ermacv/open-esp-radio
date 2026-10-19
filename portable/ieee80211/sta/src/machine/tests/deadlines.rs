use super::*;

fn assert_scan_overflow(world: &mut World<'_>, now: Instant) {
    assert_eq!(
        world.completed,
        [StaCommandTerminal {
            request: StaRequest::new(401, StaCommand::BackgroundScan),
            outcome: StaCommandOutcome::BackgroundScan {
                interrupted: Some(BackgroundScanEnd::DeadlineOverflow),
            },
        }]
    );
    assert!(world.station.is_connected());
    assert!(world.station.link().is_some());
    assert_eq!(world.arbiter.channel(), Some(channel(6)));
    assert!(world.station.absence.is_none());
    assert!(world.station.absence_place.is_some());
    assert!(world.station.configuration.is_some());
    assert!(matches!(world.station.lease, Held::Lease(_)));
    assert_eq!(world.station.tickets.len(), 2);
    assert_eq!(world.station.buffers.len(), 2);
    assert_eq!(world.station.lost_tokens(), 0);
    assert!(world.station.poll_deadline().is_none());
    assert!(world.station.poll_action(now).is_none());
    assert!(world.arbiter.poll_action(now).is_none());
    assert!(
        world.frames.is_empty(),
        "failed dwell sends no probe or deauthentication"
    );
    // A subsequent, representable operation really runs on the retained lease.
    world.completed.clear();
    world.station.config.dwell = Duration::from_micros(1);
    world
        .station
        .command(now, StaRequest::new(402, StaCommand::BackgroundScan))
        .unwrap();
    world.run(now);
    assert!(world.frames.iter().any(|frame| frame[0] == 0x40));
    world.advance();
    assert_eq!(
        world.completed,
        [StaCommandTerminal {
            request: StaRequest::new(402, StaCommand::BackgroundScan),
            outcome: StaCommandOutcome::BackgroundScan { interrupted: None },
        }]
    );
    assert!(world.station.link().is_some());
    assert_eq!(world.station.tickets.len(), 2);
    assert_eq!(world.station.buffers.len(), 2);
}

#[test]
fn background_overflow_before_configuration_has_no_effect_and_keeps_the_link() {
    for delayed_poll in [false, true] {
        installed(|mint| {
            let mut world = connected_world(mint);
            world.station.config.channels = heapless::Vec::from_slice(&[channel(6)]).unwrap();
            world.completed.clear();
            world.frames.clear();
            world.take();
            let late = Instant::from_micros(u64::MAX - 1);
            world
                .station
                .command(
                    if delayed_poll { at(50) } else { late },
                    StaRequest::new(401, StaCommand::BackgroundScan),
                )
                .unwrap();
            world.run(late);
            assert!(
                world.take().is_empty(),
                "no receive, channel or wire effects before refusal"
            );
            assert_scan_overflow(&mut world, late);
        });
    }
}

#[test]
fn delayed_widen_terminal_overflow_restores_receive_before_scan_terminal() {
    installed(|mint| {
        let mut world = connected_world(mint);
        world.station.config.channels = heapless::Vec::from_slice(&[channel(6)]).unwrap();
        world.completed.clear();
        world.frames.clear();
        world.take();
        world.hold_configured = true;
        world
            .station
            .command(at(50), StaRequest::new(401, StaCommand::BackgroundScan))
            .unwrap();
        world.run(at(50));
        assert_eq!(world.take(), [Seen::Configure("bss+scan")]);
        let late = Instant::from_micros(u64::MAX - 1);
        let widened = world.configuring.take().unwrap();
        world.station.input(late, StaInput::Configured(widened));
        world.run(late);
        assert_eq!(world.take(), [Seen::Configure("bss")]);
        assert!(
            world.completed.is_empty(),
            "cleanup still owns the accepted request"
        );
        assert!(world.station.poll_deadline().is_none());
        let narrowed = world.configuring.take().unwrap();
        world.station.input(late, StaInput::Configured(narrowed));
        world.hold_configured = false;
        world.run(late);
        assert_scan_overflow(&mut world, late);
    });
}

#[test]
fn late_authentication_terminal_overflow_finishes_connect_once_with_all_credits() {
    installed(|mint| {
        let mut world = World::new(mint, 3);
        world
            .station
            .command(at(0), StaRequest::new(403, StaCommand::Connect))
            .unwrap();
        world.run(at(0));
        world.advance();
        world.receive(at(13), &beacon());
        // Enter join and complete the first Authentication transmission.
        world.advance();
        // The fixture normally completes each frame in the same step; arrange
        // the delayed terminal using the real published context and owners.
        let late = Instant::from_micros(u64::MAX - 1);
        // Expire attempt 1, allowing attempt 2 to publish at this finite time.
        let deadline = world.station.poll_deadline().unwrap();
        world.station.input(deadline, StaInput::Tick);
        let Some(StaAction::Transmit(transmit)) = world.station.poll_action(deadline) else {
            panic!()
        };
        world.transmitted(transmit, false);
        world.frames.clear();
        world.take();
        world.end_transmissions(late);
        world.run(late);
        assert_eq!(
            world.completed,
            [StaCommandTerminal {
                request: StaRequest::new(403, StaCommand::Connect),
                outcome: StaCommandOutcome::ConnectFailed(StaDisconnectReason::DeadlineOverflow),
            }]
        );
        assert_eq!(
            world.take(),
            [
                Seen::Configure("off"),
                Seen::Notify(StaNotice::Disconnected(
                    StaDisconnectReason::DeadlineOverflow
                ))
            ]
        );
        assert!(world.frames.is_empty());
        assert!(world.station.place.is_some());
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
        assert!(world.station.poll_action(late).is_none());
        assert!(world.station.poll_deadline().is_none());
    });
}

#[test]
fn handshake_deadline_overflow_finishes_connect_without_sending_eapol() {
    for message1 in [false, true] {
        installed(|mint| {
            let mut world = World::with_groups(
                mint,
                3,
                StaSecurity::Personal {
                    password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
                    pmk: Pmk::from_bytes(PMK),
                },
                [Slot(1), Slot(2)],
            );
            world
                .station
                .command(at(0), StaRequest::new(404, StaCommand::Connect))
                .unwrap();
            world.run(at(0));
            world.advance();
            world.receive(at(13), &wpa2_beacon());
            world.advance();
            world.answer_authentication(at(30), 0);
            let late = Instant::from_micros(u64::MAX - 1);
            if message1 {
                world.receive(at(31), &association_response(0));
                let frame = RsnTxFrame::<512>::message1(
                    oer_ieee80211_mac::security::rsn::Akm::Psk,
                    LOCAL,
                    1,
                    ANONCE,
                )
                .unwrap();
                world.receive(late, &eapol_frame(frame.as_bytes()));
            } else {
                world.receive(late, &association_response(0));
            }
            assert_eq!(
                world.completed,
                [StaCommandTerminal {
                    request: StaRequest::new(404, StaCommand::Connect),
                    outcome: StaCommandOutcome::ConnectFailed(
                        StaDisconnectReason::DeadlineOverflow
                    ),
                }]
            );
            assert!(world.eapol.is_empty());
            assert!(matches!(world.station.state, State::Idle));
            assert!(world.station.place.is_some());
            assert!(world.station.keys.clear());
            assert_eq!(world.station.tickets.len(), 2);
            assert_eq!(world.station.buffers.len(), 2);
            assert!(world.station.poll_deadline().is_none());
            assert!(world.station.poll_action(late).is_none());
        });
    }
}
