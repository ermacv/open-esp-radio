use super::*;

#[test]
fn failure_withdraws_a_pending_access_point_start() {
    installed(|mint| {
        let (mut arbiter, _, _) = alone::<NoReservation>(mint);
        arbiter.want_access_point();
        arbiter.upstream(at(0), UpstreamFact::Connected(channel(6)));
        let mut failed = failed(arbiter);
        assert_eq!(
            failed_notices(&mut failed),
            [
                Seen::AccessPoint(AccessPointCommand::Stop),
                Seen::Station(StationNotice::Leave(LeaveReason::PortFailed)),
            ]
        );
        assert!(failed.poll_notice().is_none());
    });
}

#[test]
fn failure_drains_each_lease_once_at_every_gate_stage() {
    // Waiting, Closing (the backend holds the gate), Closed, Retuning,
    // and Running. Both the primary and absence stay owned until failure.
    for stage in 0..5 {
        installed(|mint| {
            let (mut arbiter, home, place, mut port) = connected(mint);
            let away = absence(arbiter.request_absence(at(0), scan(6, 10), &home, place));
            arbiter.ready(away.id());
            let now = away.from();
            let mut late_gate = None;
            let mut late_retune = None;
            if stage > 0 {
                let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(now) else {
                    panic!("begin the absence gate cycle")
                };
                if stage == 1 {
                    late_gate = Some(gate);
                } else {
                    arbiter.gate_closed(port.issuer.close(gate));
                }
            }
            if stage > 2 {
                let Some(ArbiterAction::Retune(pending)) = arbiter.poll_action(now) else {
                    panic!("begin the absence retune")
                };
                if stage == 3 {
                    late_retune = Some(pending);
                } else {
                    arbiter.retuned(pending);
                    assert_eq!(
                        run(&mut arbiter, &mut port, now),
                        [
                            Seen::OpenGate,
                            Seen::Station(StationNotice::AbsenceStarted(away.id())),
                        ]
                    );
                }
            }
            let mut failed = failed(arbiter);
            assert_eq!(
                failed_notices(&mut failed),
                [
                    Seen::AccessPoint(AccessPointCommand::Stop),
                    Seen::Station(StationNotice::AbsenceCancelled(
                        CancelReason::PortFailed,
                        absence_place()
                    )),
                    Seen::Station(StationNotice::LeaseCancelled(
                        CancelReason::PortFailed,
                        self::place()
                    )),
                    Seen::Station(StationNotice::Leave(LeaveReason::PortFailed)),
                ],
                "stage {stage}"
            );
            if let Some(gate) = late_gate {
                failed.return_closed_gate(port.issuer.close(gate));
            }
            if let Some(pending) = late_retune {
                failed.retune_ended(pending);
            }
            assert!(
                failed.poll_notice().is_none(),
                "late owners cannot begin another lifecycle"
            );
            assert!(failed.poll_notice().is_none(), "terminals are emitted once");
        });
    }
}

#[test]
fn policy_checks_the_actual_publication_budget_before_consuming_authority() {
    let budget = Duration::from_millis(3);
    let minimum = Duration::from_millis(15); // 2 * (publication 3 + retune 2) + listen 5
    let exact = ArbiterPolicy {
        absence: minimum,
        ..ArbiterPolicy::DEFAULT
    };
    assert!(exact.checked::<AirReservation>(budget).is_ok());
    let short = ArbiterPolicy {
        absence: Duration::from_micros(14_999),
        ..exact
    };
    assert_eq!(
        short.checked::<AirReservation>(budget).err(),
        Some(ArbiterPolicyError::AbsenceTooShort { needed: minimum })
    );
    let long = ArbiterPolicy {
        absence: Duration::from_micros(32_768),
        ..exact
    };
    assert_eq!(
        long.checked::<AirReservation>(budget).err(),
        Some(ArbiterPolicyError::ReservationTooLong)
    );
    assert!(long.checked::<NoReservation>(budget).is_ok());
    assert_eq!(
        exact
            .checked::<NoReservation>(Duration::from_micros(u64::MAX))
            .err(),
        Some(ArbiterPolicyError::AbsenceBudgetOverflow)
    );
    let unordered = ArbiterPolicy {
        follow: ApFollowPolicy {
            min_follow_interval: Duration::from_secs(2),
            max_follow_interval: Duration::from_secs(1),
            ..ApFollowPolicy::DEFAULT
        },
        ..exact
    };
    assert_eq!(
        unordered.checked::<NoReservation>(budget).err(),
        Some(ArbiterPolicyError::FollowIntervalOrder)
    );
    installed(|mint| {
        let (gate, _issuer, authority) = mint.into_pools();
        assert!(short.checked::<NoReservation>(budget).is_err());
        let (_arbiter, _primary, _absence) =
            Arbiter::<Caps>::new(exact.checked(budget).unwrap(), channel(1), authority, gate);
    });
}

#[test]
fn exhausted_identities_return_places_without_reusing_an_old_lease_name() {
    installed(|mint| {
        let (mut arbiter, home, absence_place, mut port) = connected(mint);
        arbiter.next_id = Some(u64::MAX);
        let final_lease = absence(arbiter.request_absence(at(0), scan(6, 5), &home, absence_place));
        assert_ne!(final_lease.id(), home.id());
        arbiter.release(final_lease);
        let absence_place = run(&mut arbiter, &mut port, at(0))
            .into_iter()
            .find_map(|event| match event {
                Seen::Station(StationNotice::AbsenceCancelled(CancelReason::Released, place)) => {
                    Some(place)
                }
                _ => None,
            })
            .unwrap();
        assert!(matches!(
            arbiter.request_absence(at(0), scan(6, 5), &home, absence_place),
            AbsenceAnswer::Refused(AbsenceRefusal::IdentityExhausted, _)
        ));
        arbiter.access_point(at(1), AccessPointFact::SwitchDue(channel(6)));
        run(&mut arbiter, &mut port, at(1));
        let old_id = home.id();
        let home = arbiter.exchange(home).unwrap_err();
        assert_eq!(home.id(), old_id);
        arbiter.release(home);
        let place = run(&mut arbiter, &mut port, at(1))
            .into_iter()
            .find_map(|event| match event {
                Seen::Station(StationNotice::LeaseEnded(place)) => Some(place),
                _ => None,
            })
            .unwrap();
        assert!(matches!(
            arbiter.request(at(1), connect(11), place),
            Answer::Refused(Refusal::IdentityExhausted, _)
        ));
        assert!(run(&mut arbiter, &mut port, at(1)).is_empty());
    });
}

#[test]
fn schedule_exhaustion_has_no_announcement_or_follow_backoff() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = alone::<NoReservation>(mint);
        let end = Instant::from_micros(u64::MAX);
        arbiter.access_point(
            end,
            AccessPointFact::Running {
                channel: channel(1),
                next_tbtt: end,
                beacon_interval: BeaconInterval::new(Duration::from_millis(100)).unwrap(),
            },
        );
        let Answer::Refused(Refusal::ScheduleExhausted, place) =
            arbiter.request(end, connect(6), place)
        else {
            panic!("unrepresentable move must return its admission place")
        };
        assert!(run(&mut arbiter, &mut port, end).is_empty());
        assert_eq!(arbiter.channel(), Some(channel(1)));
        assert_eq!(arbiter.poll_deadline(), None);
        assert_eq!(arbiter.coordinator.last_follow_for_test(), None);
        // The returned place admits an on-channel request even at the limit.
        let home = lease(arbiter.request(end, connect(1), place));
        assert_eq!(home.channel(), channel(1));
    });
}

#[test]
fn follow_without_switch_fact_cancels_once_after_one_beacon_interval() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        let pending = lease(arbiter.request(at(0), connect(6), place));
        assert_eq!(pending.from(), at(550));
        run(&mut arbiter, &mut port, at(0));
        assert_eq!(arbiter.poll_deadline(), Some(at(650)));
        assert!(run(&mut arbiter, &mut port, at(649)).is_empty());
        let events = run(&mut arbiter, &mut port, at(650));
        assert!(matches!(
            events.as_slice(),
            [Seen::Station(StationNotice::LeaseCancelled(_, _))]
        ));
        assert!(run(&mut arbiter, &mut port, at(650)).is_empty());
        assert_eq!(arbiter.poll_deadline(), None);
        assert_eq!(arbiter.channel(), Some(channel(1)));
        // A later AP terminal may still move its announced BSS, but cannot
        // resurrect the cancelled station lease or publish its Started.
        arbiter.access_point(at(651), AccessPointFact::SwitchDue(channel(6)));
        assert!(
            !run(&mut arbiter, &mut port, at(651))
                .iter()
                .any(|event| matches!(event, Seen::Station(StationNotice::LeaseStarted(_))))
        );
    });
}

#[test]
fn stopping_ap_preserves_the_promised_earliest_start() {
    installed(|mint| {
        let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
        let pending = lease(arbiter.request(at(0), connect(6), place));
        run(&mut arbiter, &mut port, at(0));
        arbiter.access_point(at(100), AccessPointFact::Stopped);
        assert!(run(&mut arbiter, &mut port, at(100)).is_empty());
        assert_eq!(arbiter.poll_deadline(), Some(pending.from()));
        assert_eq!(arbiter.channel(), Some(channel(1)));
        let events = run(&mut arbiter, &mut port, pending.from());
        assert!(
            matches!(events.last(), Some(Seen::Station(StationNotice::LeaseStarted(id))) if *id == pending.id())
        );
    });
}

#[test]
fn release_while_closing_cancels_before_outbound_retune() {
    for to in [1, 6] {
        installed(|mint| {
            let (mut arbiter, place, mut port) = alone::<NoReservation>(mint);
            let pending = lease(arbiter.request(at(0), connect(to), place));
            let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(at(0)) else {
                panic!("close outgoing gate")
            };
            arbiter.release(pending);
            arbiter.gate_closed(port.issuer.close(gate));
            assert_eq!(
                run(&mut arbiter, &mut port, at(0)),
                [
                    Seen::Station(StationNotice::LeaseCancelled(
                        CancelReason::Released,
                        self::place()
                    )),
                    Seen::OpenGate,
                ]
            );
            assert_eq!(arbiter.channel(), Some(channel(1)));
        });
    }
}

#[test]
fn fallback_restart_waits_for_the_completed_return_cycle() {
    for scan_channel in [1, 6] {
        for refuse in [false, true] {
            installed(|mint| {
                let (mut arbiter, place, mut port) = beside_access_point::<NoReservation>(mint);
                arbiter.upstream(at(0), UpstreamFact::Connected(channel(1)));
                arbiter.upstream(at(0), UpstreamFact::Lost);
                run(&mut arbiter, &mut port, at(30_000));
                let scanning = lease(arbiter.request(at(30_001), scan(scan_channel, 10), place));
                run(&mut arbiter, &mut port, at(30_001));
                run(&mut arbiter, &mut port, scanning.until().unwrap());
                arbiter.upstream(at(30_020), UpstreamFact::NotFound);
                let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(at(30_020))
                else {
                    panic!("AP restart must first complete the return cycle")
                };
                assert!(arbiter.poll_notice(at(30_020)).is_none());
                arbiter.gate_closed(port.issuer.close(gate));
                if scan_channel != 1 {
                    let Some(ArbiterAction::Retune(pending)) = arbiter.poll_action(at(30_020))
                    else {
                        panic!("return to channel 1 before starting AP")
                    };
                    assert_eq!(pending.channel(), channel(1));
                    if refuse {
                        let RetuneRefused::Failed(mut failed) = arbiter.retune_refused(pending)
                        else {
                            panic!("a refused fallback return must be terminal")
                        };
                        let events = failed_notices(&mut failed);
                        assert!(failed.poll_notice().is_none());
                        assert!(!events.iter().any(|event| matches!(
                            event,
                            Seen::AccessPoint(AccessPointCommand::Start(_))
                        )));
                        return;
                    }
                    arbiter.retuned(pending);
                    assert!(arbiter.poll_notice(at(30_020)).is_none());
                }
                let Some(ArbiterAction::OpenGate(closed)) = arbiter.poll_action(at(30_020)) else {
                    panic!("reopen returned gate before AP restart")
                };
                arbiter.gate_opened(port.issuer.open(closed));
                let events = run(&mut arbiter, &mut port, at(30_020));
                assert_eq!(events.iter().filter(|event| matches!(event, Seen::AccessPoint(AccessPointCommand::Start(to)) if *to == channel(1))).count(), 1);
            });
        }
    }
}

#[test]
fn an_application_stop_does_not_report_port_failure() {
    installed(|mint| {
        let (mut arbiter, home, place, _) = connected(mint);
        let _away = absence(arbiter.request_absence(at(0), scan(6, 10), &home, place));
        let mut stopped = arbiter.finish(ArbiterEnd::Stopped);
        let notices: Vec<_> = core::iter::from_fn(|| stopped.poll_notice()).collect();
        assert!(
            notices.iter().all(|notice| !matches!(
                notice,
                ArbiterNotice::Station(
                    StationNotice::LeaseCancelled(CancelReason::PortFailed, _)
                        | StationNotice::AbsenceCancelled(CancelReason::PortFailed, _)
                        | StationNotice::Leave(LeaveReason::PortFailed)
                )
            )),
            "Stop must retain its own terminal cause: {notices:?}"
        );
        assert_eq!(notices.len(), 4);
    });
}

#[test]
fn stopped_bank_reuses_places_and_gate_without_reusing_lease_identity() {
    for closed in [false, true] {
        installed(|mint| {
            let (gate, issuer, authority) = mint.into_pools();
            let (mut arbiter, place, absence) = Arbiter::<Caps>::new(
                ArbiterPolicy::DEFAULT.checked(Duration::ZERO).unwrap(),
                channel(1),
                authority,
                gate,
            );
            let mut port = Port { issuer };
            let first = lease(arbiter.request(at(0), scan(6, 10), place));
            arbiter.ready(first.id());
            if closed {
                let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(at(0)) else {
                    panic!()
                };
                arbiter.gate_closed(port.issuer.close(gate));
            }
            let mut retired = arbiter.finish(ArbiterEnd::Stopped);
            let mut places = StationPlaceReturns::new(None, Some(absence));
            while let Some(notice) = retired.poll_notice() {
                if let ArbiterNotice::Station(notice) = notice {
                    places.returned(notice);
                }
            }
            let RetiredArbiter::Stopped(bank) = retired else {
                panic!("Stop is reusable")
            };
            let (mut arbiter, places) = bank
                .restart(channel(1), places)
                .unwrap_or_else(|_| panic!("the gate terminal was returned"))
                .into_parts();
            let reopened = run(&mut arbiter, &mut port, at(1));
            assert_eq!(
                reopened
                    .iter()
                    .filter(|event| matches!(event, Seen::OpenGate))
                    .count(),
                usize::from(closed)
            );
            let (place, _absence) = places.into_parts();
            let second = lease(arbiter.request(at(2), scan(6, 10), place));
            assert_ne!(
                second.id(),
                first.id(),
                "an installation must never reuse an old lease ID"
            );
        });
    }
}

#[test]
fn stop_retains_a_missing_gate_terminal_instead_of_minting_authority() {
    installed(|mint| {
        let (gate, issuer, authority) = mint.into_pools();
        let (mut arbiter, place, absence) = Arbiter::<Caps>::new(
            ArbiterPolicy::DEFAULT.checked(Duration::ZERO).unwrap(),
            channel(1),
            authority,
            gate,
        );
        let mut port = Port { issuer };
        let first = lease(arbiter.request(at(0), scan(6, 10), place));
        arbiter.ready(first.id());
        let Some(ArbiterAction::CloseGate { gate, .. }) = arbiter.poll_action(at(0)) else {
            panic!()
        };
        let RetiredArbiter::Stopped(bank) = arbiter.finish(ArbiterEnd::Stopped) else {
            panic!()
        };
        let Err(IncompleteArbiter {
            bank,
            mut places,
            missing,
        }) = bank.restart(channel(1), StationPlaceReturns::new(None, Some(absence)))
        else {
            panic!("a pending gate has no restart proof")
        };
        assert_eq!(missing, MissingArbiterReturn::GateTerminal);
        let mut retired = RetiredArbiter::Stopped(bank);
        retired.return_closed_gate(port.issuer.close(gate));
        while let Some(notice) = retired.poll_notice() {
            if let ArbiterNotice::Station(notice) = notice {
                places.returned(notice);
            }
        }
        let RetiredArbiter::Stopped(bank) = retired else {
            panic!()
        };
        assert!(bank.restart(channel(1), places).is_ok());
    });
}
