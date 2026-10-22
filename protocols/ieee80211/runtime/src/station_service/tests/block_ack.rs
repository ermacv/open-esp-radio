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

async fn connected<Id: SlotIdentity>(world: &Application<'_, '_, '_, Id>, seen: &mut usize) {
    world.associate_with(seen, ht_beacon(), false).await;
    world
        .notice(StaNotice::Connected {
            bssid: Bssid::new(IndividualAddress::new(BSSID).unwrap()).unwrap(),
            channel: 6,
        })
        .await;
}

use oer_ieee80211_wire::address::Bssid;

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
            world.command(ServiceCommand::Stop).await;
        })
    });
    assert_eq!(counters.rx_frames_dropped.get(), 0);
    assert_eq!(counters.rx_msdus_queue_full.get(), 0);
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
            world.command(ServiceCommand::Stop).await;
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
                world.command(ServiceCommand::Stop).await;
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
            world.command(ServiceCommand::Stop).await;
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
            world.command(ServiceCommand::Stop).await;
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
            world.command(ServiceCommand::Stop).await;
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
            world.command(ServiceCommand::Stop).await;
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
            world.command(ServiceCommand::Stop).await;
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
            world.command(ServiceCommand::Stop).await;
        })
    });
    assert_eq!(counters.events_lost.get(), 0);
    assert_eq!(counters.rx_frames_dropped.get(), 0);
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
            world.command(ServiceCommand::Stop).await;
        })
    });
}
