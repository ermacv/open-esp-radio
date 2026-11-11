//! The data: the network's MSDUs beside the station's own frames, their
//! queues, categories and rates, and the received data's admission,
//! lending and copies.

use super::*;
use crate::station::{RxMsdusCopied, RxMsdusRefused};
use crate::test_fixtures::{NULL, QOS_DATA, WMM, carries_eapol, open_amsdu};

/// The data attempts the air saw from `from` before the first frame
/// `until` takes.
fn data_before(air: &[AirFrame], from: usize, until: impl Fn(&AirFrame) -> bool) -> usize {
    air[from..]
        .iter()
        .take_while(|frame| !until(frame))
        .filter(|frame| kind(frame) == QOS_DATA && !carries_eapol(frame))
        .count()
}

/// A beacon of the protected QoS network.
fn wmm_beacon() -> Vec<u8> {
    let mut beacon = beacon(true);
    beacon.extend_from_slice(&WMM);
    beacon
}

/// Fill the data machine and the network's queue while the data's arena
/// stays on the air: how many MSDUs of `priority` the network sent.
async fn fill(world: &World<'_>, destination: [u8; 6], priority: u8) -> usize {
    let mut sent = 0;
    while world.send_msdu_at(destination, priority) {
        sent += 1;
        world.settle().await;
    }
    sent
}

/// Data holds every token of its own and the station's frames still go:
/// with the best-effort arena held on the air, the network fills the data
/// machine and its own queue, and the group handshake's EAPOL, the Null
/// before a background scan's absence and the Deauthentication still reach
/// the air.
#[test]
fn data_in_both_arenas_leaves_eapol_null_and_deauthentication_their_tokens() {
    pair(
        || two_channels(wpa2()),
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.handshake(&mut seen).await;
                world
                    .air
                    .hold_completion(WmmAccessCategory::BestEffort, true);
                // The arena on the air, the standby's frame and the MSDU
                // that waits for it; the network's queue holds the rest.
                let sent = fill(world, [0x02, 0, 0, 0, 0, 0x07], 0).await;
                assert_eq!(sent, 3 + TX_DEPTH);
                assert_eq!(
                    world.air.sent()[seen..]
                        .iter()
                        .filter(|frame| kind(frame) == DATA)
                        .count(),
                    1,
                    "only the held arena went"
                );
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                let group_message2 = loop {
                    let frame = world.sent(&mut seen, DATA).await;
                    if frame.plaintext.get(38..40) == Some(&[0x88, 0x8e][..]) {
                        break frame;
                    }
                };
                assert_ne!(group_message2.plaintext[1] & 0x40, 0);
                world.station_command(StaCommand::BackgroundScan).await;
                let null = world.sent(&mut seen, NULL).await;
                assert_ne!(null.plaintext[1] & 0x10, 0, "PM=1 before the absence");
                world.station_command(StaCommand::Disconnect).await;
                world.sent(&mut seen, DEAUTHENTICATION).await;
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// The station's frames share the Voice queue with data of priority 6–7;
/// data holds at most its two arenas, so once a held Voice arena is
/// released EAPOL and Deauthentication go after at most that arena and the
/// standby's frame.
#[test]
fn eapol_and_deauthentication_wait_for_at_most_the_two_arenas() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let bound = 2;
                let destination = [0x02, 0, 0, 0, 0, 0x07];
                let mut seen = 0;
                world.associate_with(&mut seen, wmm_beacon(), true).await;
                let ptk = world.handshake(&mut seen).await;
                let data_after = |from: usize| {
                    world.air.sent()[from..]
                        .iter()
                        .filter(|frame| kind(frame) == QOS_DATA && !carries_eapol(frame))
                        .count()
                };
                let released = world.air.sent().len();
                world.air.hold_completion(WmmAccessCategory::Voice, true);
                // The data machine holds its arenas' frames and the network
                // its full queue.
                let sent = fill(world, destination, 6).await;
                assert!(sent > TX_DEPTH, "{sent}");
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                world.settle().await;
                world.air.hold_completion(WmmAccessCategory::Voice, false);
                while !world.air.sent()[released..].iter().any(carries_eapol) {
                    world.sent(&mut seen, QOS_DATA).await;
                }
                let before = data_before(&world.air.sent(), released, carries_eapol);
                assert!(before <= bound, "EAPOL after {before} data frames");
                // The same for the Deauthentication, the queue full again.
                while data_after(released) < sent {
                    world.sent(&mut seen, QOS_DATA).await;
                }
                let released = world.air.sent().len();
                world.air.hold_completion(WmmAccessCategory::Voice, true);
                let sent = fill(world, destination, 6).await;
                assert!(sent > TX_DEPTH, "{sent}");
                world.station_command(StaCommand::Disconnect).await;
                world.settle().await;
                world.air.hold_completion(WmmAccessCategory::Voice, false);
                world.sent(&mut seen, DEAUTHENTICATION).await;
                let before = data_before(&world.air.sent(), released, |frame| {
                    kind(frame) == DEAUTHENTICATION
                });
                assert!(
                    before <= bound,
                    "Deauthentication after {before} data frames"
                );
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// Station frames and data of several TIDs interleave their ends: each end
/// returns to its owner, the data goes in the network's order, every frame
/// teaches the rates, and both arenas and the station's tokens serve again.
#[test]
fn interleaved_station_and_data_terminals_keep_their_owner_and_the_networks_order() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate_with(&mut seen, wmm_beacon(), true).await;
                let ptk = world.handshake(&mut seen).await;
                let baseline = world.completions.borrow().len();
                // The first data arena stays on the air while the others
                // wait in their TIDs' queues: the MAC starts each next arena
                // from the highest access category, voice, then video, then
                // best effort.
                world.air.hold_completion(WmmAccessCategory::Voice, true);
                for (index, priority) in [6, 0, 7, 4].into_iter().enumerate() {
                    assert!(world.send_msdu_at([2, 0, 0, 0, 0, index as u8], priority));
                    world.settle().await;
                }
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                world.settle().await;
                world.air.hold_completion(WmmAccessCategory::Voice, false);
                let mut data = Vec::new();
                let mut eapol = 0;
                while data.len() < 4 || eapol == 0 {
                    let frame = world.sent(&mut seen, QOS_DATA).await;
                    if carries_eapol(&frame) {
                        eapol += 1;
                    } else {
                        data.push((frame.plaintext[21], frame.plaintext[24] & 0x0f));
                    }
                }
                assert_eq!(data, [(0, 6), (2, 7), (3, 4), (1, 0)]);
                // Every data frame's end teaches the rates once it came.
                let learned = |rates: FrameRates| {
                    world.completions.borrow()[baseline..]
                        .iter()
                        .filter(|learned| **learned == rates)
                        .count()
                };
                for _ in 0..20 {
                    if learned(FrameRates::Data) >= 4 {
                        break;
                    }
                    world.settle().await;
                    world.advance();
                }
                let completions = world.completions.borrow()[baseline..].to_vec();
                assert_eq!(
                    completions
                        .iter()
                        .filter(|rates| **rates == FrameRates::Data)
                        .count(),
                    4,
                    "{completions:?}"
                );
                assert!(completions.contains(&FrameRates::Control));
                // Both arenas and the station's tokens serve again.
                for priority in [0, 4, 6, 7] {
                    assert!(world.send_msdu_at([2, 0, 0, 0, 1, priority], priority));
                    let frame = world.sent(&mut seen, QOS_DATA).await;
                    assert_eq!(frame.plaintext[21], priority);
                }
                world.station_command(StaCommand::Disconnect).await;
                world.sent(&mut seen, DEAUTHENTICATION).await;
                world.stop().await;
            })
        })],
        |ends, _| {
            assert!(stopped(ends));
            assert!(
                ends[0]
                    .lifecycle
                    .as_ref()
                    .unwrap()
                    .retired
                    .rejected
                    .is_none()
            );
        },
    );
}

/// The access point closes Voice to a station without an admission: an
/// IPv4 packet marked EF (user priority 6) goes as Video, in TID 5, and so
/// does the station's own EAPOL.
#[test]
fn a_frame_marked_for_voice_under_a_closed_voice_category_goes_as_video_in_tid_five() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut wmm = WMM;
                wmm[22] |= 0x10;
                let mut beacon = beacon(true);
                beacon.extend_from_slice(&wmm);
                let mut seen = 0;
                world.associate_with(&mut seen, beacon, true).await;
                let ptk = world.handshake(&mut seen).await;
                // Voice never opens: what goes, goes in another category.
                world.air.hold(WmmAccessCategory::Voice, true);
                // An IPv4 header marked EF (DSCP 46), total length 20.
                let mut ip = [0_u8; 20];
                ip[0] = 0x45;
                ip[1] = 46 << 2;
                ip[3] = 20;
                let requested =
                    oer_protocols_bridges_ieee80211_ethernet::qos::classify_wmm(0x0800, &ip);
                assert_eq!(requested.user_priority, WmmUserPriority::UP6);
                assert!(world.send_packet(
                    oer_protocols_ieee80211_wire::data::MsduParts {
                        destination: [2, 0, 0, 0, 0, 9],
                        source: LOCAL,
                        ether_type: 0x0800,
                        payload: &ip,
                    },
                    requested.user_priority,
                ));
                let data = world.sent(&mut seen, QOS_DATA).await;
                assert!(!carries_eapol(&data));
                assert_eq!(data.plaintext[21], 9);
                assert_eq!(data.plaintext[24] & 0x0f, 5, "TID 5, not 6");
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                let response = world.sent(&mut seen, QOS_DATA).await;
                assert!(carries_eapol(&response));
                assert_eq!(response.plaintext[24] & 0x0f, 5, "TID 5, not 7");
                world.air.hold(WmmAccessCategory::Voice, false);
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// The rates plan no rate for a data frame: it stays in its arena, refused
/// to no one, and goes numbered with the next plan.
#[test]
fn a_data_frame_without_a_rate_waits_in_its_arena_and_goes_with_the_next_plan() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                world.handshake(&mut seen).await;
                world.settle().await;
                let sent_before = world.air.sent().len();
                world.refuse_rate.set(Some(FrameRates::Data));
                assert!(world.send_msdu(BSSID));
                world.settle().await;
                assert_eq!(world.air.sent().len(), sent_before, "no rate, no frame");
                assert_eq!(world.heard.tx_refused.get(), 0);
                assert_eq!(world.heard.msdu_refused.get(), 0);
                assert_eq!(world.refuse_rate.get(), None, "the plan was asked");
                // The next turn plans a rate: the frame goes, numbered.
                assert!(world.send_msdu(BSSID));
                let frame = world.sent(&mut seen, DATA).await;
                assert!(frame.packet_number.is_some());
                assert_eq!(frame.ticket, None, "data goes through an arena");
                world.sent(&mut seen, DATA).await;
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// The network's MSDU goes as one protected MPDU to the DS under the
/// pairwise key; a decrypted MPDU reaches the network lent in place, and
/// neither its replay, an unverified one nor an unprotected one does;
/// after the disconnection no data goes.
#[test]
fn protected_data_goes_both_ways_and_none_goes_after_the_disconnection() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                world.handshake(&mut seen).await;
                let destination = [0x02, 0, 0, 0, 0, 0x07];
                assert!(world.send_msdu(destination));
                let data = world.sent(&mut seen, DATA).await;
                assert!(matches!(data.key, Some((_, true))), "{data:?}");
                assert!(data.packet_number.is_some());
                assert_ne!(data.plaintext[1] & 0x40, 0);
                assert_eq!(data.plaintext[1] & 0x03, 0x01, "to the DS");
                assert_eq!(data.plaintext[4..10], BSSID);
                assert_eq!(data.plaintext[10..16], LOCAL);
                assert_eq!(data.plaintext[16..22], destination);
                assert_eq!(
                    data.plaintext[32..40],
                    [0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]
                );
                assert_eq!(data.plaintext[40..], [0xab; 100]);
                let source = [0x02, 0, 0, 0, 0, 0x08];
                world
                    .air
                    .receive(&protected_data(1, source, &[0xcd; 40]), decrypted());
                world.settle().await;
                let mut expected = Vec::new();
                expected.extend_from_slice(&LOCAL);
                expected.extend_from_slice(&source);
                expected.extend_from_slice(&[0x08, 0x00]);
                expected.extend_from_slice(&[0xcd; 40]);
                assert_eq!(world.delivered_frames(), [expected]);
                assert_eq!(world.lent.get(), 1, "its one MSDU stays lent in place");
                world
                    .air
                    .receive(&protected_data(1, source, &[0xcd; 40]), decrypted());
                world
                    .air
                    .receive(&protected_data(2, source, &[0xcd; 40]), meta());
                let mut plain = protected_data(3, source, &[0xcd; 40]);
                plain[1] &= !0x40;
                plain.drain(24..32);
                world.air.receive(&plain, meta());
                world.settle().await;
                assert_eq!(world.delivered(), 0);
                assert_eq!(world.air.pending_rx(), 0, "every buffer came back");
                world.station_command(StaCommand::Disconnect).await;
                world.sent(&mut seen, DEAUTHENTICATION).await;
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::LocalRequest))
                    .await;
                let left = world.air.sent().len();
                world.send_msdu(destination);
                for _ in 0..5 {
                    world.settle().await;
                    world.advance();
                }
                assert!(
                    world.air.sent()[left..]
                        .iter()
                        .all(|frame| kind(frame) != DATA),
                    "no data after the disconnection"
                );
                world.stop().await;
            })
        })],
        |ends, air| {
            assert!(stopped(ends));
            assert_eq!(air.pending_rx(), 0);
        },
    );
}

/// An A-MSDU's MSDUs reach the network as copies; a frame of one MSDU is
/// lent in place; every buffer comes back to the port.
#[test]
fn an_amsdu_is_copied_and_a_single_msdu_lent() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world.connected().await;
                // The actor takes the link before the access point's data.
                world.settle().await;
                let source = [0x02, 0, 0, 0, 0, 0x08];
                world
                    .air
                    .receive(&open_amsdu(source, &[&[1; 20], &[2; 30]]), meta());
                world.settle().await;
                let delivered = world.delivered_frames();
                assert_eq!(delivered.len(), 2);
                assert_eq!(delivered[0][14..], [1; 20]);
                assert_eq!(delivered[1][14..], [2; 30]);
                assert_eq!(world.lent.get(), 0, "an A-MSDU's MSDUs are copies");
                assert_eq!(
                    world.observed.service(|event| match event {
                        ServiceEvent::RxMsdusCopied(copied) => Some(copied),
                        _ => None,
                    }),
                    [RxMsdusCopied {
                        aggregated: 2,
                        shared: 0,
                        kept: 0
                    }]
                );
                world.air.receive(&open_amsdu(source, &[&[3; 20]]), meta());
                world.settle().await;
                assert_eq!(world.delivered(), 1);
                assert_eq!(world.lent.get(), 1, "one MSDU stays in its buffer");
                assert_eq!(world.air.pending_rx(), 0, "every buffer came back");
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// A composition without a network refuses a received MSDU, its buffer
/// goes back at once, and the stop ends normally.
#[test]
fn without_a_network_a_received_msdu_returns_its_buffer_and_the_stop_ends_normally() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                world.network_absent.set(true);
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world.connected().await;
                // The actor takes the link before the access point's data.
                world.settle().await;
                world
                    .air
                    .receive(&open_data(1, [0x02, 0, 0, 0, 0, 0x08]), meta());
                world.settle().await;
                assert_eq!(world.delivered(), 0, "nothing is queued");
                assert_eq!(world.air.pending_rx(), 0, "the buffer went back at once");
                assert_eq!(
                    world.observed.service(|event| match event {
                        ServiceEvent::RxMsdusRefused(refused) => Some(refused),
                        _ => None,
                    }),
                    [RxMsdusRefused {
                        no_network: 1,
                        ..RxMsdusRefused::default()
                    }]
                );
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// The access point's first data frame lands right after the lifecycle
/// handed the actor the link, before the actor's turn took the handover:
/// it reaches the network.
#[test]
fn a_data_frame_landing_with_the_handover_reaches_the_network() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world.connected().await;
                world
                    .air
                    .receive(&open_data(1, [0x02, 0, 0, 0, 0, 0x08]), meta());
                world.settle().await;
                assert_eq!(world.delivered(), 1);
                assert_eq!(world.observed.rx_frames_dropped(), 0);
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}
