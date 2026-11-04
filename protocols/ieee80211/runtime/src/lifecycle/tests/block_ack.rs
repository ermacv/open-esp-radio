//! The Block Ack agreements: the receive agreements the access point asks
//! for, which the station answers and the actor reorders under, and the
//! transmit agreements the station negotiates for the network's data.

use super::*;
use crate::test_fixtures::block_ack::{
    addba, addba_response, data, delba, ht_beacon, ht_config, management, response,
    tx_block_ack_config,
};
use crate::test_fixtures::{QOS_DATA, qos_sequence};
use oer_protocols_ieee80211_testing_model::AirAnswer;
use oer_protocols_ieee80211_wire::block_ack::frame::BlockAckAction;
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

/// An action frame of the Block Ack category.
const ACTION: u16 = 0x00d0;

/// Connected to the HT access point, the actor holding the link.
async fn connected(world: &World<'_>, seen: &mut usize) {
    world.associate_with(seen, ht_beacon(), false).await;
    world.connected().await;
    world.settle().await;
}

/// The access point asks for an agreement: the station's answer.
async fn ask(world: &World<'_>, seen: &mut usize, request: Vec<u8>) -> BlockAckAction {
    world.air.receive(&request, meta());
    response(&world.sent(seen, ACTION).await)
}

#[test]
fn exhausted_shared_bank_pool_refuses_and_peer_delba_makes_the_slot_reusable() {
    pair(
        ht_config,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                connected(world, &mut seen).await;
                for (dialog, tid) in [(1, 0), (2, 1)] {
                    assert!(matches!(
                        ask(world, &mut seen, addba(dialog, tid, 10, 0)).await,
                        BlockAckAction::AddbaResponse { status: 0, .. }
                    ));
                }
                assert_eq!(world.air.receive_agreements().len(), 2);
                world.air.receive(&data(11, 0, 11), meta());
                assert!(world.markers().await.is_empty());
                assert!(matches!(
                    ask(world, &mut seen, addba(3, 2, 10, 0)).await,
                    BlockAckAction::AddbaResponse {
                        dialog_token: 3,
                        status: 37,
                        tid: 2,
                        ..
                    }
                ));
                // A recipient's DELBA names the access point's own agreement
                // as the station's: nothing ends.
                world.air.receive(&delba(0, false), meta());
                assert!(world.markers().await.is_empty());
                assert_eq!(world.air.receive_agreements().len(), 2);
                world.air.receive(&delba(0, true), meta());
                assert_eq!(world.markers().await, [11]);
                assert_eq!(world.air.receive_agreements().len(), 1);
                assert!(matches!(
                    ask(world, &mut seen, addba(4, 2, 10, 0)).await,
                    BlockAckAction::AddbaResponse {
                        dialog_token: 4,
                        status: 0,
                        tid: 2,
                        ..
                    }
                ));
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn a_legacy_association_and_an_unsupported_tid_receive_explicit_refusals() {
    for ht in [false, true] {
        pair(
            move || if ht { ht_config() } else { open() },
            std::vec![script(move |world| {
                Box::pin(async move {
                    let tid = if ht { 15 } else { 0 };
                    let mut seen = 0;
                    world
                        .associate_with(
                            &mut seen,
                            if ht { ht_beacon() } else { beacon(false) },
                            false,
                        )
                        .await;
                    world.connected().await;
                    assert!(matches!(
                        ask(world, &mut seen, addba(7, tid, 10, 0)).await,
                        BlockAckAction::AddbaResponse {
                            dialog_token: 7,
                            status: 37,
                            tid: actual,
                            ..
                        } if actual == tid
                    ));
                    assert!(world.air.receive_agreements().is_empty());
                    world.stop().await;
                })
            })],
            |ends, _| assert!(stopped(ends)),
        );
    }
}

#[test]
fn inactivity_timer_releases_frames_and_bank_and_sends_a_recipient_delba() {
    pair(
        ht_config,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                connected(world, &mut seen).await;
                assert!(matches!(
                    ask(world, &mut seen, addba(7, 0, 10, 20)).await,
                    BlockAckAction::AddbaResponse {
                        status: 0,
                        timeout_tu: 20,
                        ..
                    }
                ));
                world.air.receive(&data(11, 0, 2), meta());
                assert!(world.markers().await.is_empty());
                world.advance_by(Duration::from_micros(20 * 1024));
                let ended = world.sent(&mut seen, ACTION).await;
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
                assert_eq!(world.markers().await, [2]);
                assert!(matches!(
                    ask(world, &mut seen, addba(8, 0, 20, 0)).await,
                    BlockAckAction::AddbaResponse { status: 0, .. }
                ));
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn a_duplicate_dialog_resends_response_without_restarting_the_reorder_window() {
    pair(
        ht_config,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                connected(world, &mut seen).await;
                let request = addba(7, 0, 10, 0);
                ask(world, &mut seen, request.clone()).await;
                world.air.receive(&data(11, 0, 2), meta());
                assert!(world.markers().await.is_empty());
                assert!(matches!(
                    ask(world, &mut seen, request).await,
                    BlockAckAction::AddbaResponse { status: 0, .. }
                ));
                assert!(
                    world.markers().await.is_empty(),
                    "the retry keeps the buffered frames"
                );
                world.air.receive(&data(10, 0, 1), meta());
                assert_eq!(world.markers().await, [1, 2]);
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn changed_parameters_under_the_same_dialog_are_refused_without_losing_buffered_frames() {
    pair(
        ht_config,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                connected(world, &mut seen).await;
                ask(world, &mut seen, addba(7, 0, 10, 0)).await;
                world.air.receive(&data(11, 0, 2), meta());
                assert!(world.markers().await.is_empty());
                let mut delayed = addba(7, 0, 10, 0);
                delayed[27] &= !2;
                for request in [addba(7, 0, 10, 1), addba(7, 0, 20, 0), delayed] {
                    assert!(matches!(
                        ask(world, &mut seen, request).await,
                        BlockAckAction::AddbaResponse {
                            dialog_token: 7,
                            status: 37,
                            ..
                        }
                    ));
                    assert!(world.markers().await.is_empty());
                    assert_eq!(world.air.receive_agreements().len(), 1);
                }
                world.air.receive(&data(10, 0, 1), meta());
                assert_eq!(world.markers().await, [1, 2]);
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn closing_a_gap_returns_every_held_receive_credit_and_the_gap_timer_is_bounded() {
    pair(
        ht_config,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                connected(world, &mut seen).await;
                ask(world, &mut seen, addba(7, 0, 10, 0)).await;
                for sequence in 11..14 {
                    world
                        .air
                        .receive(&data(sequence, 0, sequence as u8), meta());
                    assert!(world.markers().await.is_empty());
                }
                world.air.receive(&data(10, 0, 10), meta());
                assert_eq!(world.markers().await, [10, 11, 12, 13]);
                assert_eq!(world.air.pending_rx(), 0, "every held unit came back");
                world.air.receive(&data(15, 0, 15), meta());
                assert!(world.markers().await.is_empty());
                world.advance_by(Duration::from_millis(100));
                assert_eq!(world.markers().await, [15], "the gap's timer released it");
                assert_eq!(world.air.receive_agreements().len(), 1);
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn a_busy_control_transmitter_retains_a_second_tids_response() {
    pair(
        ht_config,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                connected(world, &mut seen).await;
                world.air.hold(WmmAccessCategory::Voice, true);
                world.air.receive(&addba(1, 0, 10, 0), meta());
                world.air.receive(&addba(2, 1, 10, 0), meta());
                world.settle().await;
                world.air.hold(WmmAccessCategory::Voice, false);
                let first = response(&world.sent(&mut seen, ACTION).await);
                let second = response(&world.sent(&mut seen, ACTION).await);
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
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// Thirteen requests come while the control transmitter is busy: the
/// answers keep the requests' order, none overtakes an older one, and a
/// request beyond the reply places is counted and unanswered until the
/// access point asks again.
#[test]
fn response_pressure_answers_in_order_and_counts_requests_beyond_the_reply_places() {
    pair(
        ht_config,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                connected(world, &mut seen).await;
                world.air.hold(WmmAccessCategory::Voice, true);
                for dialog in 1..=13 {
                    world.air.receive(&addba(dialog, 15, 10, 0), meta());
                    assert!(world.markers().await.is_empty());
                }
                world.air.hold(WmmAccessCategory::Voice, false);
                for _ in 0..20 {
                    world.settle().await;
                    world.advance();
                }
                let answered: Vec<u8> = world
                    .air
                    .sent()
                    .iter()
                    .filter(|frame| kind(frame) == ACTION)
                    .map(|frame| match response(frame) {
                        BlockAckAction::AddbaResponse {
                            dialog_token,
                            status: 37,
                            ..
                        } => dialog_token,
                        other => panic!("a refusal: {other:?}"),
                    })
                    .collect();
                let expected: Vec<u8> = (1..=13).take(answered.len()).collect();
                assert_eq!(answered, expected, "in the requests' order");
                let full = world
                    .observed
                    .0
                    .borrow()
                    .iter()
                    .filter(|observation| {
                        **observation == Observation::Actor(ActorEvent::RepliesFull)
                    })
                    .count();
                assert_eq!(
                    answered.len() + full,
                    13,
                    "every request answered or counted"
                );
                assert_eq!(world.observed.rx_frames_dropped(), 0);
                assert_eq!(world.air.pending_rx(), 0);
                // The access point asks again: the station answers.
                seen = world.air.sent().len();
                let again = ask(world, &mut seen, addba(13, 15, 10, 0)).await;
                assert!(
                    matches!(
                        again,
                        BlockAckAction::AddbaResponse {
                            dialog_token: 13,
                            status: 37,
                            ..
                        }
                    ),
                    "{again:?} after {answered:?}"
                );
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// The Sequence Number and Retry bit of a sent QoS Data frame.
fn sequence_and_retry(frame: &AirFrame) -> (u16, bool) {
    (qos_sequence(frame), frame.plaintext[1] & 0x08 != 0)
}

/// Connected under an operational transmit agreement of TID 0, whose
/// negotiation's MSDU took Sequence Number 0.
async fn agreed(world: &World<'_>, seen: &mut usize) {
    connected(world, seen).await;
    assert!(world.send_msdu(BSSID));
    let BlockAckAction::AddbaRequest { dialog_token, .. } =
        response(&world.sent(seen, ACTION).await)
    else {
        panic!("an ADDBA Request");
    };
    world
        .air
        .receive(&addba_response(dialog_token, 0, 32), meta());
    world.settle().await;
}

#[test]
fn msdus_queued_while_an_arena_is_out_fill_the_standby_and_a_missing_frame_retries_in_its_arena() {
    pair(
        || tx_block_ack_config(ht_config()),
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                agreed(world, &mut seen).await;
                // Sequence Number 1 goes alone and stays on the air; 2, 3
                // and 4 fill the standby meanwhile.
                world
                    .air
                    .hold_completion(WmmAccessCategory::BestEffort, true);
                for _ in 0..4 {
                    assert!(world.send_msdu(BSSID));
                    world.settle().await;
                }
                // The A-MPDU of 2, 3 and 4 misses 3.
                world.air.answer(AirAnswer::BlockAck(0b101));
                world
                    .air
                    .hold_completion(WmmAccessCategory::BestEffort, false);
                let retried = loop {
                    let frame = world.sent(&mut seen, QOS_DATA).await;
                    if sequence_and_retry(&frame).1 {
                        break frame;
                    }
                };
                // Only 3 goes again, from the same arena, with its Retry bit.
                assert_eq!(sequence_and_retry(&retried), (3, true));
                assert!(
                    world
                        .air
                        .sent()
                        .iter()
                        .filter(|frame| kind(frame) == QOS_DATA)
                        .all(|frame| ![(2, true), (4, true)].contains(&sequence_and_retry(frame)))
                );
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn a_frame_missed_by_every_attempt_is_dropped_and_a_bar_moves_the_window() {
    pair(
        || tx_block_ack_config(ht_config()),
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                agreed(world, &mut seen).await;
                // 1 stays on the air while 2 and 3 fill the standby.
                world
                    .air
                    .hold_completion(WmmAccessCategory::BestEffort, true);
                for _ in 0..3 {
                    assert!(world.send_msdu(BSSID));
                    world.settle().await;
                }
                // The policy allows four A-MPDUs on the air.
                for _ in 0..4 {
                    world.air.answer(AirAnswer::BlockAck(0));
                }
                world
                    .air
                    .hold_completion(WmmAccessCategory::BestEffort, false);
                let bar = world.sent(&mut seen, 0x0084).await;
                // A compressed BlockAckReq: its starting sequence after the
                // dropped 2 and 3.
                let control = u16::from_le_bytes([bar.plaintext[18], bar.plaintext[19]]);
                assert_eq!(control >> 4, 4);
                let tries = world
                    .air
                    .sent()
                    .iter()
                    .filter(|frame| kind(frame) == QOS_DATA && sequence_and_retry(frame).0 == 2)
                    .count();
                assert_eq!(tries, 4);
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn a_refused_negotiation_is_not_repeated_and_receive_agreements_still_answer() {
    pair(
        || tx_block_ack_config(ht_config()),
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                connected(world, &mut seen).await;
                assert!(world.send_msdu(BSSID));
                let BlockAckAction::AddbaRequest { dialog_token, .. } =
                    response(&world.sent(&mut seen, ACTION).await)
                else {
                    panic!("an ADDBA Request");
                };
                let refusal = oer_protocols_ieee80211_core_upper_mac::block_ack::AddbaResponse {
                    dialog_token,
                    status:
                        oer_protocols_ieee80211_core_upper_mac::block_ack::AddbaStatus::RequestDeclined,
                    tid: 0,
                    window: 0,
                    timeout_tu: 0,
                    amsdu: false,
                };
                world
                    .air
                    .receive(&management(&refusal.encode().unwrap()), meta());
                world.settle().await;
                assert!(world.send_msdu(BSSID));
                // The access point's own negotiation still gets its answer,
                // and it is the next action frame: the refused TID asks
                // nothing again.
                assert!(matches!(
                    ask(world, &mut seen, addba(9, 0, 4, 0)).await,
                    BlockAckAction::AddbaResponse {
                        dialog_token: 9,
                        status: 0,
                        ..
                    }
                ));
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

#[test]
fn a_station_without_an_originator_or_on_a_legacy_link_negotiates_nothing() {
    for ht in [true, false] {
        pair(
            move || {
                if ht {
                    ht_config()
                } else {
                    tx_block_ack_config(open())
                }
            },
            std::vec![script(move |world| {
                Box::pin(async move {
                    let mut seen = 0;
                    if ht {
                        connected(world, &mut seen).await;
                    } else {
                        world.associate(&mut seen, false).await;
                        world.connected().await;
                    }
                    assert!(world.send_msdu(BSSID));
                    world
                        .sent(&mut seen, if ht { QOS_DATA } else { DATA })
                        .await;
                    world.settle().await;
                    assert!(world.air.sent().iter().all(|frame| kind(frame) != ACTION));
                    world.stop().await;
                })
            })],
            |ends, _| assert!(stopped(ends)),
        );
    }
}

#[test]
fn only_a_bar_from_the_associated_transmitter_moves_its_tid_window() {
    pair(
        ht_config,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                connected(world, &mut seen).await;
                ask(world, &mut seen, addba(7, 0, 10, 0)).await;
                world.air.receive(&data(11, 0, 11), meta());
                assert!(world.markers().await.is_empty());
                let bar = oer_protocols_ieee80211_wire::block_ack::frame::encode_block_ack_request(
                    LOCAL,
                    BSSID,
                    0,
                    SequenceNumber::new(12).unwrap(),
                );
                let mut forged = bar;
                forged[10] ^= 2;
                world.air.receive(&forged, meta());
                assert!(world.markers().await.is_empty());
                world.air.receive(&bar, meta());
                assert_eq!(world.markers().await, [11]);
                world.air.receive(&data(12, 0, 12), meta());
                assert_eq!(world.markers().await, [12]);
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}
