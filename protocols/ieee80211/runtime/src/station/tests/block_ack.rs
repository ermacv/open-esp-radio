use super::*;
use oer_ieee80211_wire::{
    block_ack::frame::{
        BlockAckAction, DelbaParameters, encode_addba_request, parse_block_ack_action,
    },
    ht::{HtLocalCapabilities, ht_capability_ie},
    sequence::SequenceNumber,
};

const HT: [u8; 28] = ht_capability_ie(
    HtLocalCapabilities::new(0, 0, 0xff, 1),
    match Channel::ghz2_4(6, ChannelWidth::Mhz20) {
        Ok(channel) => channel,
        Err(_) => panic!("fixed HT20 test channel"),
    },
);
static HT_ASSOCIATION: AssociationCapabilities = AssociationCapabilities {
    ht20: HT,
    ht40: [0; 28],
    he20_ht: [0; 28],
    he20: [0; 24],
    he20_extended: [0; 14],
    wmm: [221, 7, 0, 0x50, 0xf2, 2, 0, 1, 0],
};

fn ht_config() -> TestConfig {
    let (mut configuration, security) = config(StaSecurity::Open);
    configuration.phy = PhyMode::Ht20;
    configuration.association = &HT_ASSOCIATION;
    (configuration, security)
}

fn ht_beacon() -> Vec<u8> {
    let mut frame = beacon(false);
    frame.extend_from_slice(&HT);
    frame.extend_from_slice(&HT_ASSOCIATION.wmm);
    frame.extend_from_slice(&[
        61, 22, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    frame
}

fn management(body: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0; 24];
    frame[0] = 0xd0;
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame.extend_from_slice(body);
    frame
}

fn addba(dialog: u8, tid: u8, start: u16, timeout: u16) -> Vec<u8> {
    management(
        &encode_addba_request(
            dialog,
            tid,
            64,
            timeout,
            true,
            SequenceNumber::new(start).unwrap(),
        )
        .unwrap(),
    )
}

fn delba(tid: u8, initiator: bool) -> Vec<u8> {
    let [a, b] = DelbaParameters::from_bytes([0; 2])
        .with_tid(tid)
        .unwrap()
        .with_initiator(initiator)
        .to_bytes();
    management(&[3, 2, a, b, 37, 0])
}

fn data(sequence: u16, tid: u8, marker: u8) -> Vec<u8> {
    let mut frame = std::vec![0; 26];
    frame[..2].copy_from_slice(&0x0288u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[22..24].copy_from_slice(
        &SequenceNumber::new(sequence)
            .unwrap()
            .sequence_control()
            .to_le_bytes(),
    );
    frame[24] = tid;
    frame.extend_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 8, 0, marker]);
    frame
}

fn response(frame: &AirFrame) -> BlockAckAction {
    parse_block_ack_action(&frame.plaintext[24..]).unwrap()
}

async fn connected(world: &Application<'_, '_, '_>, seen: &mut usize) {
    world.associate_with(seen, ht_beacon(), false).await;
    world
        .notice(StaNotice::Connected {
            bssid: BSSID,
            channel: 6,
        })
        .await;
}

#[test]
fn addba_programs_an_owned_bank_before_response_and_reorders_downlink() {
    let counters = run_with(ht_config(), |world| {
        Box::pin(async move {
            let mut seen = 0;
            connected(world, &mut seen).await;
            world.air.receive(&addba(7, 0, 4094, 0), meta());
            let ack = world.sent(&mut seen, 0xd0).await;
            assert!(matches!(
                response(&ack),
                BlockAckAction::AddbaResponse {
                    dialog_token: 7,
                    status: 0,
                    tid: 0,
                    window: 4,
                    amsdu: true,
                    ..
                }
            ));
            assert_eq!(world.air.receive_agreements().len(), 1);
            world.air.receive(&data(4095, 0, 2), meta());
            assert!(world.delivered().await.is_empty());
            world.air.receive(&data(4094, 0, 1), meta());
            let delivered = world.delivered().await;
            assert_eq!(
                delivered
                    .iter()
                    .map(|frame| *frame.last().unwrap())
                    .collect::<Vec<_>>(),
                [1, 2]
            );
            world.air.receive(&data(4095, 0, 2), meta());
            assert!(
                world.delivered().await.is_empty(),
                "an old retry must only recycle its RX owner"
            );
            world.air.receive(&data(0, 0, 3), meta());
            assert_eq!(world.delivered().await[0].last(), Some(&3));
            world.command(ControlCommand::Stop).await;
        })
    });
    assert_eq!(counters.rx_frames_dropped(), 0);
    assert!(counters.rx_msdus_refused().is_empty());
}

/// Ten MSDUs reach a network that takes four lent frames at once and
/// returns them only when the test drains it. The station holds four more
/// back in order; the model's ring keeps nothing beyond the station's four
/// units, so the last two are lost there, before the station, as on air.
fn held_back<'w>(world: &'w Application<'w, '_, '_>) -> Script<'w> {
    Box::pin(async move {
        let mut seen = 0;
        connected(world, &mut seen).await;
        for sequence in 1..=10 {
            world
                .air
                .receive(&data(sequence, 0, sequence as u8), meta());
            for _ in 0..20 {
                yield_now().await;
            }
        }
        let first = world.delivered().await;
        assert_eq!(first.len(), MODEL_RX_EVENTS, "the network's lend limit");
        let mut delivered = first;
        // Each drain returns the lent frames; the waiting ones follow.
        for _ in 0..10 {
            if delivered.len() >= 2 * MODEL_RX_EVENTS {
                break;
            }
            delivered.extend(world.delivered().await);
        }
        assert_eq!(
            delivered
                .iter()
                .map(|frame| frame.last().copied())
                .collect::<Vec<_>>(),
            (1..=8u8).map(Some).collect::<Vec<_>>(),
            "every MSDU the station took reaches the network once, in order"
        );
        world.command(ControlCommand::Stop).await;
    })
}

#[test]
fn a_full_network_holds_frames_back_without_copy_or_refusal() {
    let counters = run_with(ht_config(), held_back);
    assert_eq!(counters.rx_frames_dropped(), 0);
    assert!(counters.rx_msdus_refused().is_empty());
    assert!(counters.rx_msdus_copied().is_empty());
}

#[test]
fn the_empty_sink_holds_frames_back_as_an_enabled_one() {
    let enabled = run_with(ht_config(), held_back);
    assert!(enabled.rx_msdus_refused().is_empty());
    let empty = run_with_empty_sink(ht_config(), held_back);
    assert!(empty.events.lock().unwrap().is_empty());
}

#[test]
fn exhausted_shared_bank_pool_refuses_and_peer_delba_makes_the_slot_reusable() {
    run_with(ht_config(), |world| {
        Box::pin(async move {
            let mut seen = 0;
            connected(world, &mut seen).await;
            for (dialog, tid) in [(1, 0), (2, 1)] {
                world.air.receive(&addba(dialog, tid, 10, 0), meta());
                assert!(matches!(
                    response(&world.sent(&mut seen, 0xd0).await),
                    BlockAckAction::AddbaResponse { status: 0, .. }
                ));
            }
            assert_eq!(world.air.receive_agreements().len(), 2);
            world.air.receive(&data(11, 0, 11), meta());
            assert!(world.delivered().await.is_empty());
            world.air.receive(&addba(3, 2, 10, 0), meta());
            assert!(matches!(
                response(&world.sent(&mut seen, 0xd0).await),
                BlockAckAction::AddbaResponse {
                    dialog_token: 3,
                    status: 37,
                    tid: 2,
                    ..
                }
            ));
            world.air.receive(&delba(0, false), meta());
            let _ = world.delivered().await;
            assert_eq!(world.air.receive_agreements().len(), 2);
            world.air.receive(&delba(0, true), meta());
            assert_eq!(world.delivered().await[0].last(), Some(&11));
            assert_eq!(world.air.receive_agreements().len(), 1);
            world.air.receive(&addba(4, 2, 10, 0), meta());
            assert!(matches!(
                response(&world.sent(&mut seen, 0xd0).await),
                BlockAckAction::AddbaResponse {
                    dialog_token: 4,
                    status: 0,
                    tid: 2,
                    ..
                }
            ));
            world.command(ControlCommand::Stop).await;
        })
    });
}

#[test]
fn a_legacy_association_and_an_unsupported_tid_receive_explicit_refusals() {
    for (configuration, ht, tid) in [
        (config(StaSecurity::Open), false, 0),
        (ht_config(), true, 15),
    ] {
        run_with(configuration, |world| {
            Box::pin(async move {
                let mut seen = 0;
                world
                    .associate_with(
                        &mut seen,
                        if ht { ht_beacon() } else { beacon(false) },
                        false,
                    )
                    .await;
                world.air.receive(&addba(7, tid, 10, 0), meta());
                assert!(matches!(response(&world.sent(&mut seen, 0xd0).await),
                BlockAckAction::AddbaResponse { dialog_token: 7, status: 37, tid: actual, .. } if actual == tid));
                assert!(world.air.receive_agreements().is_empty());
                world.command(ControlCommand::Stop).await;
            })
        });
    }
}

#[test]
fn inactivity_timer_releases_frames_and_bank_and_sends_a_recipient_delba() {
    run_with(ht_config(), |world| {
        Box::pin(async move {
            let mut seen = 0;
            connected(world, &mut seen).await;
            world.air.receive(&addba(7, 0, 10, 20), meta());
            assert!(matches!(
                response(&world.sent(&mut seen, 0xd0).await),
                BlockAckAction::AddbaResponse {
                    status: 0,
                    timeout_tu: 20,
                    ..
                }
            ));
            world.air.receive(&data(11, 0, 2), meta());
            assert!(world.delivered().await.is_empty());
            world
                .clock
                .advance(Duration::from_micros(20 * 1024))
                .unwrap();
            let ended = world.sent(&mut seen, 0xd0).await;
            assert!(matches!(
                response(&ended),
                BlockAckAction::Delba {
                    tid: 0,
                    initiator: false,
                    reason: 39
                }
            ));
            assert_eq!(ended.plaintext.len(), 30);
            assert!(world.air.receive_agreements().is_empty());
            assert_eq!(world.delivered().await[0].last(), Some(&2));
            world.air.receive(&addba(8, 0, 20, 0), meta());
            assert!(matches!(
                response(&world.sent(&mut seen, 0xd0).await),
                BlockAckAction::AddbaResponse { status: 0, .. }
            ));
            world.command(ControlCommand::Stop).await;
        })
    });
}

#[test]
fn a_duplicate_dialog_resends_response_without_restarting_the_reorder_window() {
    run_with(ht_config(), |world| {
        Box::pin(async move {
            let mut seen = 0;
            connected(world, &mut seen).await;
            let request = addba(7, 0, 10, 0);
            world.air.receive(&request, meta());
            let _ = world.sent(&mut seen, 0xd0).await;
            world.air.receive(&data(11, 0, 2), meta());
            assert!(world.delivered().await.is_empty());
            world.air.receive(&request, meta());
            assert!(matches!(
                response(&world.sent(&mut seen, 0xd0).await),
                BlockAckAction::AddbaResponse { status: 0, .. }
            ));
            assert!(
                world.delivered().await.is_empty(),
                "retry must not tear down buffered frames"
            );
            world.air.receive(&data(10, 0, 1), meta());
            assert_eq!(
                world
                    .delivered()
                    .await
                    .iter()
                    .map(|frame| *frame.last().unwrap())
                    .collect::<Vec<_>>(),
                [1, 2]
            );
            world.command(ControlCommand::Stop).await;
        })
    });
}

#[test]
fn changed_parameters_under_the_same_dialog_are_refused_without_losing_buffered_frames() {
    run_with(ht_config(), |world| {
        Box::pin(async move {
            let mut seen = 0;
            connected(world, &mut seen).await;
            world.air.receive(&addba(7, 0, 10, 0), meta());
            let _ = world.sent(&mut seen, 0xd0).await;
            world.air.receive(&data(11, 0, 2), meta());
            assert!(world.delivered().await.is_empty());
            let mut delayed = addba(7, 0, 10, 0);
            delayed[27] &= !2;
            for request in [addba(7, 0, 10, 1), addba(7, 0, 20, 0), delayed] {
                world.air.receive(&request, meta());
                assert!(matches!(
                    response(&world.sent(&mut seen, 0xd0).await),
                    BlockAckAction::AddbaResponse {
                        dialog_token: 7,
                        status: 37,
                        ..
                    }
                ));
                assert!(world.delivered().await.is_empty());
                assert_eq!(world.air.receive_agreements().len(), 1);
            }
            world.air.receive(&data(10, 0, 1), meta());
            assert_eq!(
                world
                    .delivered()
                    .await
                    .iter()
                    .map(|frame| *frame.last().unwrap())
                    .collect::<Vec<_>>(),
                [1, 2]
            );
            world.command(ControlCommand::Stop).await;
        })
    });
}

#[test]
fn closing_a_gap_returns_every_held_receive_credit_and_the_gap_timer_is_bounded() {
    run_with(ht_config(), |world| {
        Box::pin(async move {
            let mut seen = 0;
            connected(world, &mut seen).await;
            world.air.receive(&addba(7, 0, 10, 0), meta());
            let _ = world.sent(&mut seen, 0xd0).await;
            // The model has four outstanding RX credits. Release each receive
            // event to the service before injecting another, just as a live
            // port applies its credit limit.
            for sequence in 11..14 {
                world
                    .air
                    .receive(&data(sequence, 0, sequence as u8), meta());
                assert!(world.delivered().await.is_empty());
            }
            world.air.receive(&data(10, 0, 10), meta());
            assert_eq!(
                world
                    .delivered()
                    .await
                    .iter()
                    .map(|frame| *frame.last().unwrap())
                    .collect::<Vec<_>>(),
                [10, 11, 12, 13]
            );
            world.air.receive(&data(15, 0, 15), meta());
            assert!(world.delivered().await.is_empty());
            world.clock.advance(Duration::from_millis(100)).unwrap();
            assert_eq!(world.delivered().await[0].last(), Some(&15));
            assert_eq!(world.air.receive_agreements().len(), 1);
            world.command(ControlCommand::Stop).await;
        })
    });
}

#[test]
fn a_busy_control_transmitter_retains_a_second_tids_response() {
    run_with(ht_config(), |world| {
        Box::pin(async move {
            let mut seen = 0;
            connected(world, &mut seen).await;
            world.air.hold(WmmAccessCategory::Voice, true);
            world.air.receive(&addba(1, 0, 10, 0), meta());
            world.air.receive(&addba(2, 1, 10, 0), meta());
            for _ in 0..50 {
                yield_now().await;
            }
            world.air.hold(WmmAccessCategory::Voice, false);
            let first = response(&world.sent(&mut seen, 0xd0).await);
            let second = response(&world.sent(&mut seen, 0xd0).await);
            assert!(matches!(
                first,
                BlockAckAction::AddbaResponse {
                    dialog_token: 1,
                    status: 0,
                    ..
                }
            ));
            assert!(matches!(
                second,
                BlockAckAction::AddbaResponse {
                    dialog_token: 2,
                    status: 0,
                    ..
                }
            ));
            world.command(ControlCommand::Stop).await;
        })
    });
}

#[test]
fn response_pressure_defers_management_without_overtaking_older_requests() {
    let counters = run_with(ht_config(), |world| {
        Box::pin(async move {
            let mut seen = 0;
            connected(world, &mut seen).await;
            world.air.hold(WmmAccessCategory::Voice, true);
            // One in-flight reply, eight pending refusals and four raw RX
            // owners exercise both queues without exceeding model RX credits.
            for dialog in 1..=13 {
                world.air.receive(&addba(dialog, 15, 10, 0), meta());
                assert!(world.delivered().await.is_empty());
            }
            world.air.hold(WmmAccessCategory::Voice, false);
            for expected in 1..=13 {
                assert!(matches!(response(&world.sent(&mut seen, 0xd0).await),
                BlockAckAction::AddbaResponse { dialog_token, status: 37, .. } if dialog_token == expected));
            }
            world.command(ControlCommand::Stop).await;
        })
    });
    assert_eq!(counters.port_events_lost(), 0);
    assert_eq!(counters.rx_frames_dropped(), 0);
}

/// An HT station with a transmit originator whose agreements end after one
/// idle second.
fn tx_block_ack_config((mut configuration, security): TestConfig) -> TestConfig {
    configuration.tx_block_ack = Some(
        oer_ieee80211_core_roles_sta::machine::StaTxBlockAck::new(
            oer_ieee80211_core_roles_sta::machine::StaTxBlockAckPolicy {
                window: 32,
                negotiation_timeout: Duration::from_millis(100),
                retry: oer_ieee80211_core_upper_mac::block_ack::TxBlockAckRetry {
                    attempts: 2,
                    interval: Duration::from_millis(50),
                },
                inactivity: Duration::from_secs(1),
                retry_members: oer_ieee80211_core_roles_sta::data::AggregateRetry {
                    lifetime: Duration::from_millis(1_500),
                    attempts: 4,
                },
            },
        )
        .unwrap(),
    );
    (configuration, security)
}

fn addba_response(dialog_token: u8, tid: u8, window: u16) -> Vec<u8> {
    management(
        &oer_ieee80211_core_upper_mac::block_ack::AddbaResponse {
            dialog_token,
            status: oer_ieee80211_core_upper_mac::block_ack::AddbaStatus::Success,
            tid,
            window,
            timeout_tu: 0,
            amsdu: false,
        }
        .encode()
        .unwrap(),
    )
}

#[test]
fn data_negotiates_a_transmit_agreement_that_ends_idle_with_a_delba() {
    run_with(tx_block_ack_config(ht_config()), |world| {
        Box::pin(async move {
            let mut seen = 0;
            connected(world, &mut seen).await;
            world.send_msdu(BSSID);
            let request = world.sent(&mut seen, 0xd0).await;
            let BlockAckAction::AddbaRequest {
                dialog_token,
                tid: 0,
                immediate: true,
                window: 32,
                timeout_tu: 0,
                starting_sequence,
                ..
            } = response(&request)
            else {
                panic!("an ADDBA Request of TID 0: {:?}", response(&request));
            };
            // The MSDU that started it took sequence number 0 before.
            assert_eq!(starting_sequence.get(), 1);
            world
                .air
                .receive(&addba_response(dialog_token, 0, 32), meta());
            // No more data: one idle second later the agreement ends.
            let delba = world.sent(&mut seen, 0xd0).await;
            assert_eq!(
                response(&delba),
                BlockAckAction::Delba {
                    tid: 0,
                    initiator: true,
                    reason: 39,
                }
            );
            // New data negotiates again.
            world.send_msdu(BSSID);
            assert!(matches!(
                response(&world.sent(&mut seen, 0xd0).await),
                BlockAckAction::AddbaRequest { tid: 0, .. }
            ));
            world.command(ControlCommand::Stop).await;
        })
    });
}

/// The Sequence Number and Retry bit of a sent QoS Data frame.
fn sequence_and_retry(frame: &AirFrame) -> (u16, bool) {
    let control = u16::from_le_bytes([frame.plaintext[22], frame.plaintext[23]]);
    (control >> 4, frame.plaintext[1] & 0x08 != 0)
}

#[test]
fn an_agreement_aggregates_waiting_msdus_and_retries_a_missing_member() {
    run_with(tx_block_ack_config(ht_config()), |world| {
        Box::pin(async move {
            let mut seen = 0;
            connected(world, &mut seen).await;
            world.send_msdu(BSSID);
            let BlockAckAction::AddbaRequest { dialog_token, .. } =
                response(&world.sent(&mut seen, 0xd0).await)
            else {
                panic!("an ADDBA Request");
            };
            world
                .air
                .receive(&addba_response(dialog_token, 0, 32), meta());
            assert!(world.delivered().await.is_empty());
            // Sequence number 1 goes at once; 2, 3 and 4 wait while it is
            // out, to go together.
            world.air.hold(WmmAccessCategory::BestEffort, true);
            for _ in 0..4 {
                assert!(world.try_send_msdu(BSSID));
                assert!(world.delivered().await.is_empty());
            }
            world
                .air
                .answer(oer_ieee80211_testing_model::AirAnswer::BlockAck(0b1));
            // The aggregate of 2, 3 and 4 misses 3.
            world
                .air
                .answer(oer_ieee80211_testing_model::AirAnswer::BlockAck(0b101));
            world.air.hold(WmmAccessCategory::BestEffort, false);
            let retried = loop {
                let frame = world.sent(&mut seen, 0x88).await;
                if sequence_and_retry(&frame).1 {
                    break frame;
                }
            };
            // Bit 1 of the second answer is the second subframe of the
            // second publication: 2, 3 and 4 went as one aggregate, and only
            // 3 again.
            assert_eq!(sequence_and_retry(&retried), (3, true));
            assert!(
                world
                    .air
                    .sent()
                    .iter()
                    .filter(|frame| kind(frame) == 0x88)
                    .all(|frame| ![(2, true), (4, true)].contains(&sequence_and_retry(frame)))
            );
            world.command(ControlCommand::Stop).await;
        })
    });
}

#[test]
fn a_member_missed_by_every_attempt_is_dropped_and_a_bar_moves_the_window() {
    run_with(tx_block_ack_config(ht_config()), |world| {
        Box::pin(async move {
            let mut seen = 0;
            connected(world, &mut seen).await;
            world.send_msdu(BSSID);
            let BlockAckAction::AddbaRequest { dialog_token, .. } =
                response(&world.sent(&mut seen, 0xd0).await)
            else {
                panic!("an ADDBA Request");
            };
            world
                .air
                .receive(&addba_response(dialog_token, 0, 32), meta());
            assert!(world.delivered().await.is_empty());
            // The policy allows four aggregates on the air.
            for _ in 0..4 {
                world
                    .air
                    .answer(oer_ieee80211_testing_model::AirAnswer::BlockAck(0));
            }
            world.send_msdu(BSSID);
            let bar = world.sent(&mut seen, 0x84).await;
            // Compressed BlockAckReq: its starting sequence after the dropped 1.
            let control = u16::from_le_bytes([bar.plaintext[18], bar.plaintext[19]]);
            assert_eq!(control >> 4, 2);
            let tries = world
                .air
                .sent()
                .iter()
                .filter(|frame| kind(frame) == 0x88 && sequence_and_retry(frame).0 == 1)
                .count();
            assert_eq!(tries, 4);
            world.command(ControlCommand::Stop).await;
        })
    });
}

#[test]
fn a_refused_negotiation_is_not_repeated_and_receive_agreements_still_answer() {
    run_with(tx_block_ack_config(ht_config()), |world| {
        Box::pin(async move {
            let mut seen = 0;
            connected(world, &mut seen).await;
            world.send_msdu(BSSID);
            let BlockAckAction::AddbaRequest { dialog_token, .. } =
                response(&world.sent(&mut seen, 0xd0).await)
            else {
                panic!("an ADDBA Request");
            };
            let refusal = oer_ieee80211_core_upper_mac::block_ack::AddbaResponse {
                dialog_token,
                status: oer_ieee80211_core_upper_mac::block_ack::AddbaStatus::RequestDeclined,
                tid: 0,
                window: 0,
                timeout_tu: 0,
                amsdu: false,
            };
            world
                .air
                .receive(&management(&refusal.encode().unwrap()), meta());
            world.send_msdu(BSSID);
            // The AP's own negotiation still gets its answer, and it is the
            // next action frame: the refused TID asks nothing again.
            world.air.receive(&addba(9, 0, 4, 0), meta());
            assert!(matches!(
                response(&world.sent(&mut seen, 0xd0).await),
                BlockAckAction::AddbaResponse {
                    dialog_token: 9,
                    status: 0,
                    ..
                }
            ));
            world.command(ControlCommand::Stop).await;
        })
    });
}

#[test]
fn a_station_without_an_originator_or_on_a_legacy_link_negotiates_nothing() {
    for (config, ht) in [
        (ht_config(), true),
        (tx_block_ack_config(config(StaSecurity::Open)), false),
    ] {
        run_with(config, move |world| {
            Box::pin(async move {
                let mut seen = 0;
                if ht {
                    connected(world, &mut seen).await;
                } else {
                    world.associate(&mut seen, false).await;
                }
                world.send_msdu(BSSID);
                let _ = world.sent(&mut seen, if ht { 0x88 } else { DATA }).await;
                assert!(world.delivered().await.is_empty());
                assert!(world.air.sent().iter().all(|frame| kind(frame) != 0xd0));
                world.command(ControlCommand::Stop).await;
            })
        });
    }
}

#[test]
fn only_a_bar_from_the_associated_transmitter_moves_its_tid_window() {
    run_with(ht_config(), |world| {
        Box::pin(async move {
            let mut seen = 0;
            connected(world, &mut seen).await;
            world.air.receive(&addba(7, 0, 10, 0), meta());
            let _ = world.sent(&mut seen, 0xd0).await;
            world.air.receive(&data(11, 0, 11), meta());
            assert!(world.delivered().await.is_empty());
            let bar = oer_ieee80211_wire::block_ack::frame::encode_block_ack_request(
                LOCAL,
                BSSID,
                0,
                SequenceNumber::new(12).unwrap(),
            );
            let mut forged = bar;
            forged[10] ^= 2;
            world.air.receive(&forged, meta());
            assert!(world.delivered().await.is_empty());
            world.air.receive(&bar, meta());
            assert_eq!(world.delivered().await[0].last(), Some(&11));
            world.air.receive(&data(12, 0, 12), meta());
            assert_eq!(world.delivered().await[0].last(), Some(&12));
            world.command(ControlCommand::Stop).await;
        })
    });
}
