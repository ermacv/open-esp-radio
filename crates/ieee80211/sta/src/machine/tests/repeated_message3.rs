use super::*;
use oer_ieee80211_mac::ccmp::{CcmpHeader, CcmpPacketNumber, CcmpRxProtection};

fn exchange_terminals() -> [Outcome; 5] {
    use oer_ieee80211_lower_mac::{IndeterminateCause, WithdrawCause};
    [
        ended(End::Delivered),
        ended(End::AckTimeout),
        ended(End::ContentionLimit),
        TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout),
        TxOutcome::Withdrawn(WithdrawCause::GateClosed),
    ]
}

fn response_counters(world: &World<'_>, from: usize) -> Vec<u64> {
    world.frames[from..]
        .iter()
        .map(|frame| {
            let protected = frame[1] & 0x40 != 0;
            let qos = frame[0] & 0x80 != 0;
            let start = 32 + usize::from(qos) * 2 + usize::from(protected) * 8;
            let end = frame.len();
            oer_ieee80211_rsn::EapolKeyFrame::parse(&frame[start..end])
                .unwrap()
                .replay_counter()
        })
        .collect()
}

#[test]
fn deferred_m3_responds_only_to_the_latest_counter_after_the_current_m4_terminal() {
    for connected in [false, true] {
        for counters in [&[4][..], &[4, 5][..]] {
            for outcome in exchange_terminals() {
                installed(|mint| {
                    let (mut world, ptk) = if connected {
                        wpa2_connected::<2>(mint)
                    } else {
                        wpa2_associated::<2>(mint)
                    };
                    world.hold_transmissions = true;
                    world.receive(at(50), &retry_message3(&ptk, 3, 0x99));
                    assert_eq!(world.submitted.len(), 1);
                    let before = world.frames.len();
                    for counter in counters {
                        world.receive(at(51), &retry_message3(&ptk, *counter, 0x99));
                    }
                    assert_eq!(world.frames.len(), before);
                    world.take();
                    world.outcomes.push_back((outcome, true));
                    world.end_transmissions(at(52));
                    world.run(at(52));
                    assert_eq!(
                        response_counters(&world, before),
                        [*counters.last().unwrap()]
                    );
                    assert_eq!(
                        world.station.is_connected(),
                        connected || outcome == ended(End::Delivered)
                    );
                    assert!(
                        world
                            .seen
                            .iter()
                            .all(|event| !matches!(event, Seen::InstallPairwise(_)))
                    );
                    world.hold_transmissions = false;
                    world.run(at(53));
                    assert!(world.station.is_connected());
                    assert_eq!(world.station.tickets.len(), 2);
                    assert_eq!(world.station.buffers.len(), 2);
                });
            }
        }
    }
}

#[test]
fn deferred_gm1_responds_only_to_the_latest_counter_after_the_current_gm2_terminal() {
    for counters in [&[4][..], &[4, 5][..]] {
        for outcome in exchange_terminals() {
            installed(|mint| {
                let (mut world, ptk) = wpa2_connected::<2>(mint);
                world.hold_transmissions = true;
                world.receive_verified(at(50), &group_message1(&ptk, 3, 2));
                let before = world.frames.len();
                for counter in counters {
                    world.receive_verified(at(51), &group_message1(&ptk, *counter, 2));
                }
                assert_eq!(world.frames.len(), before);
                world.outcomes.push_back((outcome, true));
                world.end_transmissions(at(52));
                world.run(at(52));
                assert_eq!(
                    response_counters(&world, before),
                    [*counters.last().unwrap()]
                );
                world.hold_transmissions = false;
                world.run(at(53));
                assert!(world.station.is_connected());
                assert_eq!(world.station.tickets.len(), 2);
                assert_eq!(world.station.buffers.len(), 2);
            });
        }
    }
}

#[test]
fn deferred_kinds_follow_the_shared_replay_order_without_dropping_either_frame() {
    for group_first in [true, false] {
        for outcome in exchange_terminals() {
            installed(|mint| {
                let (mut world, ptk) = wpa2_connected::<2>(mint);
                world.hold_transmissions = true;
                world.receive_verified(at(50), &group_message1(&ptk, 3, 2));
                let before = world.frames.len();
                if group_first {
                    world.receive_verified(at(51), &group_message1(&ptk, 4, 2));
                    world.receive(at(51), &retry_message3(&ptk, 5, 0x99));
                } else {
                    world.receive(at(51), &retry_message3(&ptk, 4, 0x99));
                    world.receive_verified(at(51), &group_message1(&ptk, 5, 2));
                }
                assert_eq!(world.frames.len(), before);
                assert_eq!(security_drops(&world), 0);
                world.outcomes.push_back((outcome, true));
                world.end_transmissions(at(52));
                world.run(at(52));
                assert_eq!(response_counters(&world, before), [4]);
                world.outcomes.push_back((outcome, true));
                world.end_transmissions(at(53));
                world.run(at(53));
                assert_eq!(response_counters(&world, before), [4, 5]);
                assert_eq!(security_drops(&world), 0);
                world.hold_transmissions = false;
                world.run(at(54));
                assert!(world.station.is_connected());
                assert!(world.station.poll_deadline().is_none());
                assert_eq!(world.station.tickets.len(), 2);
                assert_eq!(world.station.buffers.len(), 2);
            });
        }
    }
}

fn security_drops(world: &World<'_>) -> u32 {
    match &world.station.state {
        State::Connected(Connected { rsn: Some(rsn), .. })
        | State::Confirming(Confirming {
            connected: Connected { rsn: Some(rsn), .. },
            ..
        }) => rsn.dropped.get(),
        _ => panic!("the security exchange must stay alive after a protocol drop"),
    }
}

#[test]
fn invalid_mic_and_non_increasing_counters_never_replace_a_deferred_frame() {
    for group in [false, true] {
        for initial in [false, true] {
            installed(|mint| {
                let (mut world, ptk) = if initial {
                    wpa2_associated::<2>(mint)
                } else {
                    wpa2_connected::<2>(mint)
                };
                world.hold_transmissions = true;
                world.receive(at(50), &retry_message3(&ptk, 3, 0x99));
                let before = world.frames.len();
                let candidate = |counter| {
                    if group {
                        let mut frame = group_message1(&ptk, counter, 2);
                        // Vary CCMP's independent frontier separately from the
                        // EAPOL replay counter being tested.
                        frame.drain(24..32);
                        frame[1] &= !0x40;
                        frame
                    } else {
                        retry_message3(&ptk, counter, 0x99)
                    }
                };
                world.receive_verified(at(51), &protected_received(candidate(5), 100));
                let mut bad_mic = candidate(7);
                *bad_mic.last_mut().unwrap() ^= 1;
                for (index, (frame, reason)) in [
                    (bad_mic, RsnConnectedSupplicantError::InvalidMic),
                    (
                        candidate(4),
                        RsnConnectedSupplicantError::StaleReplayCounter,
                    ),
                    (
                        candidate(5),
                        RsnConnectedSupplicantError::StaleReplayCounter,
                    ),
                ]
                .into_iter()
                .enumerate()
                {
                    assert_eq!(
                        world.station.receive(
                            at(51),
                            &protected_received(frame, 101 + index as u64),
                            verified_meta(),
                            |_| {}
                        ),
                        Err(StaRxError::Security(RsnConnectedProcessError::Supplicant(
                            reason
                        )))
                    );
                    assert_eq!(security_drops(&world), 1 + index as u32);
                }
                assert_eq!(world.frames.len(), before);
                world.end_transmissions(at(52));
                world.run(at(52));
                assert_eq!(response_counters(&world, before), [5]);
                assert_eq!(security_drops(&world), 3);
            });
        }
    }
}

#[test]
fn connected_deferral_has_no_station_timeout_and_waits_for_the_port_terminal() {
    for group in [false, true] {
        installed(|mint| {
            let (mut world, ptk) = wpa2_connected::<2>(mint);
            world.hold_transmissions = true;
            world.receive(at(50), &retry_message3(&ptk, 3, 0x99));
            if group {
                world.receive_verified(at(51), &group_message1(&ptk, 4, 2));
            } else {
                world.receive(at(51), &retry_message3(&ptk, 4, 0x99));
            }
            let before = world.frames.len();
            world.take();
            assert!(world.station.poll_deadline().is_none());
            let late = Instant::from_micros(u64::MAX);
            world.station.input(late, StaInput::Tick);
            world.run(late);
            assert!(world.station.is_connected());
            assert!(world.take().is_empty());
            assert_eq!(world.frames.len(), before);
            world.outcomes.push_back((
                TxOutcome::Indeterminate(
                    oer_ieee80211_lower_mac::IndeterminateCause::HardwareTimeout,
                ),
                true,
            ));
            world.end_transmissions(late);
            world.run(late);
            assert_eq!(response_counters(&world, before), [4]);
            assert!(world.station.is_connected());
        });
    }
}

#[test]
fn original_d4_deadline_with_a_deferred_m3_fails_once_even_if_the_old_m4_later_delivers() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_associated::<2>(mint);
        let deadline = world.station.poll_deadline().unwrap();
        world.hold_transmissions = true;
        world.receive(at(50), &retry_message3(&ptk, 3, 0x99));
        for counter in [4, 5] {
            world.receive(at(51), &retry_message3(&ptk, counter, 0x99));
            assert_eq!(world.station.poll_deadline(), Some(deadline));
        }
        let before = world.frames.len();
        world.station.input(deadline, StaInput::Tick);
        world.hold_transmissions = false;
        world.run(deadline);
        for _ in 0..2 {
            world.station.input(deadline, StaInput::Tick);
            world.run(deadline);
        }
        assert!(!world.station.is_connected());
        assert_eq!(
            world
                .completed
                .iter()
                .filter(|terminal| terminal.outcome
                    == StaCommandOutcome::ConnectFailed(StaDisconnectReason::HandshakeFailed))
                .count(),
            1
        );
        assert_eq!(
            world
                .seen
                .iter()
                .filter(|event| **event
                    == Seen::Notify(StaNotice::Disconnected(
                        StaDisconnectReason::HandshakeFailed
                    )))
                .count(),
            1
        );
        assert!(
            world.frames[before..]
                .iter()
                .all(|frame| frame[0] & 0xfc == 0xc0)
        );
        assert!(world.station.keys.clear());
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
    });
}

#[test]
fn disabled_returns_both_deferred_frame_owners_with_the_station_resources() {
    for initial in [false, true] {
        installed(|mint| {
            let (mut world, ptk) = if initial {
                wpa2_associated::<2>(mint)
            } else {
                wpa2_connected::<2>(mint)
            };
            world.hold_transmissions = true;
            world.receive(at(50), &retry_message3(&ptk, 3, 0x99));
            world.receive(at(51), &retry_message3(&ptk, 4, 0x99));
            world.receive_verified(at(51), &group_message1(&ptk, 5, 2));
            let before = world.frames.len();
            world.outcomes.push_back((
                TxOutcome::Withdrawn(oer_ieee80211_lower_mac::WithdrawCause::Disabled),
                true,
            ));
            world.end_transmissions(at(52));
            world.run(at(52));
            assert!(matches!(
                world.station.state,
                State::Leave(Leave {
                    step: LeaveStep::Disabled,
                    ..
                })
            ));
            assert_eq!(world.frames.len(), before);
            let (bank, _places) = world.station.finish(StaDisconnectReason::Requested);
            assert_eq!(bank.frame_credits(), (2, 2));
            assert_eq!(bank.key_owners(), (1, 2, true));
            let (_bank, deferred, _notice) = bank.into_outputs();
            let DeferredEapol::Both { message3, group } = deferred else {
                panic!("disable must return both affine owners")
            };
            for (frame, counter) in [(message3.into_frame(), 4), (group.into_frame(), 5)] {
                assert_eq!(frame.key_frame().replay_counter(), counter);
                assert!(frame.key_frame().verify_mic(&ptk));
            }
        });
    }
}

#[test]
fn authenticated_higher_m3_keeps_ptk_and_answers_with_matching_mac_protection() {
    for protected in [false, true] {
        for material in [0x99, 0x55] {
            installed(|mint| {
                let (mut world, ptk) = wpa2_connected::<2>(mint);
                let header = CcmpHeader::new(
                    CcmpPacketNumber::new(900).unwrap(),
                    GroupKeyId::new(1).unwrap().key_id(),
                );
                let State::Connected(Connected { rsn: Some(rsn), .. }) = &mut world.station.state
                else {
                    panic!()
                };
                rsn.replay
                    .check(CcmpRxProtection::Group, None, header)
                    .unwrap();
                let before = world.frames.len();
                let frame = retry_message3(&ptk, 3, material);
                if protected {
                    world.receive_verified(at(50), &protected_received(frame, 901));
                } else {
                    world.receive(at(50), &frame);
                }
                assert!(world.station.is_connected());
                assert_eq!(
                    world.frames.len(),
                    before + 1,
                    "higher authenticated M3 needs M4"
                );
                assert_eq!(world.eapol.last(), Some(&(protected, protected)));
                assert!(
                    !world
                        .seen
                        .iter()
                        .any(|event| matches!(event, Seen::InstallPairwise(_)))
                );
                assert!(
                    world
                        .station
                        .keys
                        .groups
                        .same_material(GroupKeyId::new(1).unwrap(), &[0x99; 16])
                );
                let State::Connected(Connected { rsn: Some(rsn), .. }) = &mut world.station.state
                else {
                    panic!()
                };
                assert_eq!(rsn.supplicant.replay_counter(), 3);
                if material == 0x99 {
                    assert!(
                        rsn.replay
                            .check(CcmpRxProtection::Group, None, header)
                            .is_err()
                    );
                } else {
                    assert!(
                        rsn.replay
                            .check(CcmpRxProtection::Group, None, header)
                            .is_ok()
                    );
                }
                assert_eq!(world.station.tickets.len(), 2);
                assert_eq!(world.station.buffers.len(), 2);
            });
        }
    }
}

#[test]
fn exact_m3_reply_uses_the_incoming_mac_protection_even_with_an_installed_ptk() {
    for protected in [false, true] {
        installed(|mint| {
            let (mut world, ptk) = wpa2_connected::<2>(mint);
            let before = world.frames.len();
            let frame = retry_message3(&ptk, 2, 0x99);
            if protected {
                world.receive_verified(at(50), &protected_received(frame, 901));
            } else {
                world.receive(at(50), &frame);
            }
            assert_eq!(world.frames.len(), before + 1);
            assert_eq!(world.eapol.last(), Some(&(protected, protected)));
            assert!(world.seen.iter().all(|event| !matches!(
                event,
                Seen::InstallPairwise(_)
                    | Seen::InstallGroup(_)
                    | Seen::ReplaceGroup(_)
                    | Seen::RemoveKey
            )));
        });
    }
}

#[test]
fn failed_initial_m4_answers_a_protected_repeat_with_a_protected_m4() {
    for material in [0x99, 0x55] {
        installed(|mint| {
            let (mut world, ptk) = wpa2_associated::<2>(mint);
            world.outcomes.push_back((ended(End::AckTimeout), true));
            world.receive(at(33), &retry_message3(&ptk, 2, 0x99));
            assert!(!world.station.is_connected());
            world.take();
            world.receive_verified(
                at(34),
                &protected_received(retry_message3(&ptk, 3, material), 901),
            );
            assert!(world.station.is_connected());
            assert_eq!(world.eapol.last(), Some(&(true, true)));
            assert!(
                world
                    .seen
                    .iter()
                    .all(|event| !matches!(event, Seen::InstallPairwise(_)))
            );
        });
    }
}

#[test]
fn a_removal_terminal_preserves_an_encoded_group_response_waiting_for_credit() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        let (pending, slot, permission) = take_group_removal(&mut world, &ptk, 3, 2, at(50));
        let buffers = [
            world.station.buffers.take().unwrap(),
            world.station.buffers.take().unwrap(),
        ];
        world.receive_verified(at(51), &group_message1(&ptk, 4, 2));
        let before = world.frames.len();
        world.station.input(
            at(52),
            StaInput::GroupKeyRemoved {
                pending,
                slot,
                permission,
            },
        );
        for buffer in buffers {
            world.station.buffers.put(buffer);
        }
        world.run(at(52));
        assert_eq!(world.frames.len(), before + 1);
        assert_eq!(world.eapol.last(), Some(&(true, true)));
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
    });
}

#[test]
fn every_capacity_retains_keys_then_evicts_the_oldest_non_current_installation() {
    fn exercise<const GROUPS: usize>() {
        installed(|mint| {
            let (mut world, ptk) = wpa2_connected::<GROUPS>(mint);
            let id = GroupKeyId::new(1).unwrap();
            let mut counter = 3;
            for index in 1..GROUPS {
                let material = 10 + u8::try_from(index).unwrap();
                world.receive(at(50), &retry_message3(&ptk, counter, material));
                counter += 1;
                assert_eq!(world.station.keys.groups.len(), index + 1);
                assert!(world.station.keys.groups.same_material(id, &[0x99; 16]));
            }
            assert_eq!(world.station.keys.group_slots.free.len(), 0);
            assert!(
                world
                    .seen
                    .iter()
                    .all(|event| !matches!(event, Seen::RemoveKey | Seen::ReplaceGroup(_)))
            );
            if GROUPS > 1 {
                // Returning to old material and back must not renew its age.
                world.take();
                let latest = 10 + u8::try_from(GROUPS - 1).unwrap();
                for material in [0x99, latest] {
                    world.receive(at(51), &retry_message3(&ptk, counter, material));
                    counter += 1;
                    assert!(
                        world
                            .station
                            .keys
                            .groups
                            .current_material(id, &[material; 16])
                    );
                }
                assert_eq!(
                    world
                        .seen
                        .iter()
                        .filter(|event| matches!(event, Seen::SelectGroup))
                        .count(),
                    2
                );
                assert!(
                    world.seen.iter().all(|event| !matches!(
                        event,
                        Seen::InstallGroup(_) | Seen::ReplaceGroup(_)
                    ))
                );
            }
            world.take();
            world.receive(at(52), &retry_message3(&ptk, counter, 0xaa));
            assert_eq!(world.station.keys.groups.len(), GROUPS);
            assert!(!world.station.keys.groups.same_material(id, &[0x99; 16]));
            assert!(world.station.keys.groups.current_material(id, &[0xaa; 16]));
            for index in 1..GROUPS {
                assert!(
                    world
                        .station
                        .keys
                        .groups
                        .same_material(id, &[10 + u8::try_from(index).unwrap(); 16])
                );
            }
            assert_eq!(
                world
                    .seen
                    .iter()
                    .filter(|event| matches!(event, Seen::ReplaceGroup(_)))
                    .count(),
                1
            );
            assert!(
                world
                    .seen
                    .iter()
                    .all(|event| !matches!(event, Seen::RemoveKey | Seen::InstallPairwise(_)))
            );
        });
    }
    exercise::<1>();
    exercise::<2>();
    exercise::<3>();
    exercise::<4>();
}
