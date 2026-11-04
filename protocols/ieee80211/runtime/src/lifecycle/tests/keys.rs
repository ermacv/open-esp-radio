//! The keys: the handshake's refused Message 4, the group rekey's
//! admission, a one-slot replacement, the group replay lanes, management
//! frame protection, the key lent before the link and a new pairwise key
//! after a reconnection.

use super::*;
use crate::station::RxDrop;
use oer_protocols_ieee80211_core_roles_sta::machine::StaRxError;
use oer_protocols_ieee80211_core_upper_mac::receive::RxDropReason;

fn dropped(reason: RxDropReason) -> Observation {
    Observation::Service(ServiceEvent::RxFrameDropped(RxDrop::Station(
        StaRxError::Frame(reason),
    )))
}

/// The actor has no rate for Message 4: the frame comes back refused, the
/// handshake fails without a Connected notice, the station leaves with
/// only its Deauthentication, and only that frame teaches the rates. The
/// handshake's resources come back: the next connect handshakes again.
#[test]
fn a_message4_without_a_rate_fails_the_handshake_and_the_station_connects_again() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.start_handshake(&mut seen).await;
                world.settle().await;
                let sent_before = world.air.sent().len();
                let learned_before = world.completions.borrow().len();
                world.refuse_rate.set(Some(FrameRates::Control));
                world.air.receive(&message3(&ptk), meta());
                world
                    .notice(StaNotice::Disconnected(
                        StaDisconnectReason::HandshakeFailed,
                    ))
                    .await;
                assert!(
                    world
                        .heard
                        .refusals
                        .borrow()
                        .contains(&crate::station::contract::TransmitRefusal::NoRate),
                    "{:?}",
                    world.heard.refusals.borrow()
                );
                assert!(
                    !world
                        .heard
                        .notices
                        .borrow()
                        .iter()
                        .any(|notice| matches!(notice, StaNotice::Connected { .. }))
                );
                world.settle().await;
                let sent = world.air.sent();
                assert!(
                    sent[sent_before..]
                        .iter()
                        .all(|frame| kind(frame) == DEAUTHENTICATION),
                    "only the leave's Deauthentication went"
                );
                assert_eq!(
                    world.completions.borrow().len() - learned_before,
                    sent.len() - sent_before,
                    "only the frames that went teach the rates"
                );
                let mut seen = sent.len();
                world.associate(&mut seen, true).await;
                world.handshake(&mut seen).await;
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// A group Message 1 in a protected QoS frame from an HT access point
/// rekeys only once its integrity is verified, and once: an unverified one
/// and its replay are dropped, the replay sends no second Message 2.
#[test]
fn a_protected_qos_group_message_rekeys_once_after_integrity_and_replay_admission() {
    use oer_protocols_ieee80211_wire::ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber};
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.handshake(&mut seen).await;
                let plain = crate::test_fixtures::group_message1(&ptk, 3, 2);
                let mut protected = plain[..24].to_vec();
                protected[..2].copy_from_slice(&0xc288_u16.to_le_bytes());
                protected.extend_from_slice(&[7, 0, 0, 0, 0, 0]);
                protected.extend_from_slice(
                    &CcmpHeader::new(
                        CcmpPacketNumber::new(1).unwrap(),
                        CcmpKeyId::new(0).unwrap(),
                    )
                    .encode(),
                );
                protected.extend_from_slice(&plain[24..]);
                world.air.receive(&protected, meta());
                world.settle().await;
                assert!(world.observed.contains(dropped(RxDropReason::Integrity)));
                world.air.receive(&protected, decrypted());
                let reply = world.sent(&mut seen, DATA).await;
                assert!(reply.key.is_some());
                let header =
                    oer_protocols_ieee80211_wire::frame::DecodedHeader::decode(&reply.plaintext)
                        .unwrap();
                assert_eq!(header.body()[14..16], [0x88, 0x8e]);
                let after = world.air.sent().len();
                world.air.receive(&protected, decrypted());
                world.settle().await;
                assert_eq!(world.air.sent().len(), after, "no second Message 2");
                assert!(world.observed.contains(dropped(RxDropReason::Replay)));
                assert_eq!(world.observed.rx_frames_dropped(), 2);
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// A station with one group slot replaces its installed group key in one
/// operation, answers under the link's key and leaves normally.
#[test]
fn a_one_slot_station_replaces_its_group_key() {
    pair_with::<1>(
        wpa2,
        |_| {},
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.handshake(&mut seen).await;
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                let response = world.sent(&mut seen, DATA).await;
                assert!(matches!(response.key, Some((_, true))));
                assert!(world.observed.contains(Observation::Service(
                    ServiceEvent::GroupKeyInstalled {
                        key_id: 2,
                        replaced: true,
                    }
                )));
                world.station_command(StaCommand::Disconnect).await;
                world.sent(&mut seen, DEAUTHENTICATION).await;
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::LocalRequest))
                    .await;
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

/// A group-addressed protected data frame under group key `key_id`, as the
/// hardware hands it up decrypted, with packet number `packet_number`.
fn group_data(packet_number: u8, key_id: u8) -> Vec<u8> {
    let mut frame = protected_data(packet_number, [0x02, 0, 0, 0, 0, 0x08], &[0xcd; 40]);
    frame[4..10].copy_from_slice(&[0xff; 6]);
    frame[27] = 0x20 | (key_id << 6);
    frame
}

/// A group key's replay lane switches with its installation's terminal:
/// group data under the new key ID before the terminal is not delivered,
/// after it it is, and the installed key's lane admits each packet number
/// once.
#[test]
fn a_group_key_s_replay_lane_switches_with_its_installation_terminal() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                let ptk = world.handshake(&mut seen).await;
                // Message 3 installed group key 1.
                world.air.receive(&group_data(5, 1), decrypted());
                world.settle().await;
                assert_eq!(world.delivered(), 1);
                world.air.receive(&group_data(5, 1), decrypted());
                world.settle().await;
                assert_eq!(world.delivered(), 0, "a replay");
                // The rekey to key 2 waits at the port's barrier.
                world.air.hold_group_commands(true);
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                world.settle().await;
                world.air.receive(&group_data(1, 2), decrypted());
                world.settle().await;
                assert_eq!(world.delivered(), 0, "no lane before the terminal");
                world.air.hold_group_commands(false);
                world.sent(&mut seen, DATA).await;
                world.settle().await;
                world.air.receive(&group_data(1, 2), decrypted());
                world.settle().await;
                assert_eq!(world.delivered(), 1, "the terminal opened the lane");
                world.air.receive(&group_data(1, 2), decrypted());
                world.settle().await;
                assert_eq!(world.delivered(), 0, "a replay");
                world.stop().await;
            })
        })],
        |ends, air| {
            assert!(stopped(ends));
            assert_eq!(air.pending_rx(), 0);
        },
    );
}

/// An RSN element with management frame protection capable.
fn pmf_rsn() -> [u8; 22] {
    let mut rsn = crate::test_fixtures::RSN;
    rsn[20] |= 0x80;
    rsn
}

/// A beacon advertising management frame protection.
fn pmf_beacon() -> Vec<u8> {
    let rsn = crate::test_fixtures::RSN;
    let mut advertised = beacon(true);
    let start = advertised
        .windows(rsn.len())
        .position(|bytes| bytes == rsn)
        .unwrap();
    advertised[start..start + rsn.len()].copy_from_slice(&pmf_rsn());
    advertised
}

/// Message 3 of a PMF association: the GTK and the IGTK.
fn pmf_message3(ptk: &oer_protocols_ieee80211_core_security_rsn::Ptk) -> Vec<u8> {
    let mut plain = Vec::from(pmf_rsn());
    plain.extend_from_slice(&[0xdd, 22, 0, 0x0f, 0xac, 1, 1, 0]);
    plain.extend_from_slice(&[0x99; 16]);
    plain.extend_from_slice(&[0xdd, 28, 0, 0x0f, 0xac, 9, 4, 0, 1, 0, 0, 0, 0, 0]);
    plain.extend_from_slice(&[0x77; 16]);
    plain.extend_from_slice(&[0xdd, 0, 0, 0]);
    let wrapped = oer_protocols_ieee80211_core_security_cipher::aes::software_aes128_key_wrap(
        ptk.kek(),
        &plain,
    )
    .unwrap();
    let m3 = RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
        .unwrap()
        .authenticate(
            ptk,
            &mut oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128,
        )
        .unwrap();
    eapol_frame(m3.as_bytes())
}

/// With management frame protection negotiated, an unprotected
/// Deauthentication only starts an SA Query, which goes protected under
/// the link's key; a protected Deauthentication of the access point
/// disconnects, and the station's own goes under the key the actor kept
/// after the link came back.
#[test]
fn negotiated_pmf_checks_received_management_and_keeps_the_tx_key_through_disconnect() {
    for peer_disconnect in [false, true] {
        pair(
            wpa2,
            std::vec![script(move |world| {
                Box::pin(async move {
                    let mut seen = 0;
                    world.associate_with(&mut seen, pmf_beacon(), true).await;
                    let ptk = world.start_handshake(&mut seen).await;
                    world.air.receive(&pmf_message3(&ptk), meta());
                    let _message4 = world.sent(&mut seen, DATA).await;
                    world.connected().await;
                    let mut forged = std::vec![0; 26];
                    forged[0] = 0xc0;
                    forged[4..10].copy_from_slice(&LOCAL);
                    forged[10..16].copy_from_slice(&BSSID);
                    forged[16..22].copy_from_slice(&BSSID);
                    forged[24] = 7;
                    world.air.receive(&forged, decrypted());
                    let query = world.sent(&mut seen, 0x00d0).await;
                    assert_eq!(query.plaintext[..2], [0xd0, 0x40]);
                    assert_eq!(query.plaintext[32..34], [8, 0], "an SA Query");
                    assert!(matches!(query.key, Some((_, true))));
                    assert_eq!(query.packet_number, Some(1));
                    assert_eq!(world.heard.disconnections(), 0);
                    if peer_disconnect {
                        // An independent AES-CCM fixture under the same test
                        // PTK: the management nonce and packet number 20.
                        let hex = "c040000002000012345630055c11223330055c112233000014000020000000003f30f05c26540ce829d1";
                        let valid: Vec<_> = hex
                            .as_bytes()
                            .chunks_exact(2)
                            .map(|pair| {
                                u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap()
                            })
                            .collect();
                        world.air.receive(&valid, meta());
                        world
                            .notice(StaNotice::Disconnected(StaDisconnectReason::Peer {
                                reason_code: 7,
                            }))
                            .await;
                    } else {
                        world.station_command(StaCommand::Disconnect).await;
                        let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
                        assert_eq!(deauthentication.plaintext[..2], [0xc0, 0x40]);
                        assert!(matches!(deauthentication.key, Some((_, true))));
                        assert_eq!(deauthentication.packet_number, Some(2));
                        world
                            .notice(StaNotice::Disconnected(StaDisconnectReason::LocalRequest))
                            .await;
                    }
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
}

/// With management frame protection negotiated, a disconnection while the
/// group key's installation waits, before the link exists, sends its
/// Deauthentication protected under the installed pairwise key the
/// lifecycle lends the actor for that frame, and the key comes back.
#[test]
fn a_protected_frame_before_the_link_goes_under_the_lent_pairwise_key() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate_with(&mut seen, pmf_beacon(), true).await;
                let ptk = world.start_handshake(&mut seen).await;
                world.air.hold_group_commands(true);
                world.air.receive(&pmf_message3(&ptk), meta());
                world.settle().await;
                world.station_command(StaCommand::Disconnect).await;
                world.settle().await;
                world.air.hold_group_commands(false);
                let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
                assert_eq!(deauthentication.plaintext[..2], [0xc0, 0x40]);
                assert!(
                    matches!(deauthentication.key, Some((_, true))),
                    "{deauthentication:?}"
                );
                assert_eq!(deauthentication.packet_number, Some(1));
                // Before the link: no Message 4 went, and no connection.
                assert_eq!(
                    world
                        .air
                        .sent()
                        .iter()
                        .filter(|frame| kind(frame) == DATA)
                        .count(),
                    1,
                    "only Message 2"
                );
                assert!(
                    !world
                        .heard
                        .notices
                        .borrow()
                        .iter()
                        .any(|notice| matches!(notice, StaNotice::Connected { .. }))
                );
                world.settle().await;
                world.stop().await;
            })
        })],
        |ends, _| {
            assert!(
                stopped(ends),
                "{:?}",
                ends[0].lifecycle.as_ref().map(|c| &c.end)
            );
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

/// A reconnection installs a new pairwise key: the old link's data under
/// its key never goes after the station left, and the new link's data
/// draws its packet numbers from the new key, from the first.
#[test]
fn a_reconnection_numbers_its_data_under_the_new_pairwise_key() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, true).await;
                world.handshake(&mut seen).await;
                for _ in 0..2 {
                    assert!(world.send_msdu([0x02, 0, 0, 0, 0, 0x07]));
                    let data = world.sent(&mut seen, DATA).await;
                    assert!(matches!(data.key, Some((_, true))));
                }
                // The old link's frame waits on the air while it leaves.
                world
                    .air
                    .hold_completion(WmmAccessCategory::BestEffort, true);
                assert!(world.send_msdu([0x02, 0, 0, 0, 0, 0x0a]));
                world.settle().await;
                world.station_command(StaCommand::Disconnect).await;
                world.settle().await;
                world
                    .air
                    .hold_completion(WmmAccessCategory::BestEffort, false);
                world.sent(&mut seen, DEAUTHENTICATION).await;
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::LocalRequest))
                    .await;
                let mut seen = world.air.sent().len();
                world.associate(&mut seen, true).await;
                world.handshake(&mut seen).await;
                assert!(world.send_msdu([0x02, 0, 0, 0, 0, 0x0b]));
                let data = world.sent(&mut seen, DATA).await;
                assert_eq!(data.plaintext[16..22], [0x02, 0, 0, 0, 0, 0x0b]);
                assert!(matches!(data.key, Some((_, true))), "{data:?}");
                assert_eq!(data.packet_number, Some(1), "the new key's first number");
                world.stop().await;
            })
        })],
        |ends, air| {
            assert!(stopped(ends));
            let sent = air.sent();
            let left = sent
                .iter()
                .position(|frame| kind(frame) == DEAUTHENTICATION)
                .unwrap();
            assert!(
                !sent[left..].iter().any(|frame| kind(frame) == DATA
                    && frame.plaintext[16..22] == [0x02, 0, 0, 0, 0, 0x0a]),
                "no frame of the old link went after it left"
            );
        },
    );
}
