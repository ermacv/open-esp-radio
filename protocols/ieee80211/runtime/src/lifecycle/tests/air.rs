//! The air: the rates of the join, the handshake and the link, the
//! station's deadlines, a busy receiver, a burst of frames, background
//! scans' absences and the access point beside the station.

use super::*;
use crate::station::contract::AccessPointFact;
use crate::test_fixtures::{NULL, dsss1};
use oer_protocols_ieee80211_contracts_port::{EdcaParameterSet, SignalMetadata as _, VifRole};
use oer_protocols_ieee80211_core_upper_mac::phy_mode::PhyMode;
use oer_protocols_ieee80211_core_upper_mac::rate_control::RateControl as _;
use oer_protocols_ieee80211_wire::phy::{LegacyRate, PhyRate};

/// On an open network the join goes on the default control record, 1 Mb/s
/// DSSS; a BSS without WMM has the station contend by the DCF; the data
/// goes at the rate control's rate for the association and the next
/// control frame on the association's record.
#[test]
fn on_an_open_network_the_join_goes_on_the_default_record_and_then_the_associations() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                let [probe, authentication, association] = world.associate(&mut seen, false).await;
                assert_eq!(probe.rate, dsss1());
                assert_eq!(authentication.rate, dsss1());
                assert_eq!(association.rate, dsss1());
                world.connected().await;
                world.settle().await;
                assert_eq!(
                    world.air.edca(VifRole::Station),
                    EdcaParameterSet::from_wmm(
                        oer_protocols_ieee80211_wire::extensions::wmm::WmmParameterSet::DCF
                    )
                );
                assert_eq!(
                    world.air.edca(VifRole::AccessPoint),
                    EdcaParameterSet::STANDARD_DEFAULT
                );
                assert!(world.send_msdu([0x02, 0, 0, 0, 0, 0x07]));
                let data = world.sent(&mut seen, DATA).await;
                let peer = oer_protocols_ieee80211_core_upper_mac::rate_control::RatePeer {
                    phy: PhyMode::Legacy,
                    legacy_rates: oer_protocols_ieee80211_core_upper_mac::rate_control::NegotiatedLegacyRates::from_rate_elements(
                        &[0x82, 0x84, 0x8b, 0x96],
                        &[],
                    )
                    .unwrap(),
                    ht_capabilities: None,
                    he_capabilities: None,
                    he_peer_state: None,
                };
                assert_eq!(
                    data.rate,
                    oer_chips_family_radio_ieee80211_policy::rate_control::EspressifRateControl::for_peer(
                        (),
                        &peer,
                        meta().link_metric()
                    )
                    .mpdu_rate()
                );
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// On a protected network the join goes on the default record and the
/// handshake on the association's B-only record; Group Message 2 goes
/// protected under the link's key on that record too.
#[test]
fn on_wpa2_the_join_goes_on_the_default_record_and_the_handshake_on_the_associations() {
    pair(
        wpa2,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                let [_, authentication, association] = world.associate(&mut seen, true).await;
                assert_eq!(authentication.rate, dsss1());
                assert_eq!(association.rate, dsss1());
                let message1 = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 1, ANONCE).unwrap();
                world.air.receive(&eapol_frame(message1.as_bytes()), meta());
                let message2 = world.sent(&mut seen, DATA).await;
                assert_eq!(message2.rate, dsss1());
                let ptk = Pmk::from_bytes(PMK).derive_ptk(
                    Akm::Psk,
                    PtkContext {
                        authenticator_address: BSSID,
                        supplicant_address: LOCAL,
                        authenticator_nonce: ANONCE,
                        supplicant_nonce: SNONCE,
                    },
                );
                world.air.receive(&message3(&ptk), meta());
                let message4 = world.sent(&mut seen, DATA).await;
                assert_eq!(message4.rate, dsss1());
                world.connected().await;
                world
                    .air
                    .receive(&protected_group_message1(&ptk, 3, 2), decrypted());
                let group_message2 = world.sent(&mut seen, DATA).await;
                assert!(matches!(group_message2.key, Some((_, true))));
                assert_ne!(group_message2.plaintext[1] & 0x40, 0);
                assert!(group_message2.packet_number.is_some());
                assert_eq!(group_message2.rate, dsss1());
                world.station_command(StaCommand::Disconnect).await;
                let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
                assert_eq!(deauthentication.rate, dsss1());
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// An access point's extended OFDM rates reach both plans: data at 54
/// Mb/s, the station's control frames (the Null before a background
/// scan's absence) at the basic 6 Mb/s.
#[test]
fn an_extended_ofdm_rate_set_reaches_the_data_and_control_plans() {
    pair(
        || two_channels(open()),
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                let mut advertisement = beacon(false);
                advertisement.extend_from_slice(&[50, 8, 12, 18, 24, 36, 48, 72, 96, 108]);
                world.associate_with(&mut seen, advertisement, false).await;
                world.connected().await;
                assert!(world.send_msdu([0x02, 0, 0, 0, 0, 0x07]));
                let data = world.sent(&mut seen, DATA).await;
                assert_eq!(data.rate, PhyRate::Legacy(LegacyRate::Ofdm54M));
                world.station_command(StaCommand::BackgroundScan).await;
                let null = world.sent(&mut seen, NULL).await;
                assert_eq!(null.rate, PhyRate::Legacy(LegacyRate::Ofdm6M));
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// A station deadline reaches the station: an Authentication without a
/// response goes again once its wait passed.
#[test]
fn a_passed_station_deadline_ticks_the_station() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.station_command(StaCommand::Connect).await;
                world.sent(&mut seen, PROBE_REQUEST).await;
                world.air.receive(&beacon(false), meta());
                world.sent(&mut seen, AUTHENTICATION).await;
                // No response: the wait passes and the station tries again.
                world.sent(&mut seen, AUTHENTICATION).await;
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// A receiver that never runs dry does not starve the network's data or
/// the application's commands.
#[test]
fn continuous_rx_cannot_starve_data_tx_or_application_commands() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world.connected().await;
                let first = world.air.sent().len();
                world.air.receive_continuously(&beacon(false), meta());
                assert!(world.send_msdu(BSSID));
                world.command(ControlCommand::WantAccessPoint).await;
                let mut progressed = false;
                for _ in 0..64 {
                    yield_now().await;
                    let tx = world.air.sent()[first..]
                        .iter()
                        .any(|frame| kind(frame) == DATA);
                    let command = world
                        .heard
                        .access_point
                        .borrow()
                        .iter()
                        .any(|command| matches!(command, AccessPointCommand::Start(_)));
                    if tx && command {
                        progressed = true;
                        break;
                    }
                }
                assert!(
                    progressed,
                    "the received frames left data and commands their turn"
                );
                world.air.stop_continuous_receive();
                world.settle().await;
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// More frames for the station than its lane and the port's lend hold
/// arrive at once: they wait in the ring, nothing is lost or dropped,
/// and the station keeps carrying data.
#[test]
fn a_burst_of_station_frames_beyond_the_lane_leaves_the_station_running() {
    pair(
        open,
        std::vec![script(|world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world.connected().await;
                for _ in 0..oer_protocols_ieee80211_testing_model::MODEL_RX_EVENTS + 5 {
                    world.air.receive(&beacon(false), meta());
                }
                world.settle().await;
                assert!(world.send_msdu(BSSID));
                let data = world.sent(&mut seen, DATA).await;
                assert_eq!(data.plaintext[4..10], BSSID);
                world.settle().await;
                assert_eq!(world.air.pending_rx(), 0, "every frame came back");
                assert_eq!(world.observed.rx_frames_dropped(), 0);
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// The data queue belongs to the link: a disconnection during a background
/// scan's absence, while the port holds the link's arena, leaves none of
/// its frames to go after the reconnection, and both arenas carry the new
/// link's data.
#[test]
fn a_disconnection_during_an_absence_leaves_no_old_frame_to_go_after_the_reconnection() {
    const OLD: [u8; 6] = [0x02, 0, 0, 0, 0, 0x0a];
    const NEW: [u8; 6] = [0x02, 0, 0, 0, 0, 0x0b];
    let to = |destination: [u8; 6]| {
        move |frame: &AirFrame| kind(frame) == DATA && frame.plaintext[16..22] == destination
    };
    pair(
        || two_channels(open()),
        std::vec![script(move |world| {
            Box::pin(async move {
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world.connected().await;
                // The link's arena stays on the air; the standby holds the
                // next MPDU and one MSDU waits for it.
                world
                    .air
                    .hold_completion(WmmAccessCategory::BestEffort, true);
                for _ in 0..3 {
                    assert!(world.send_msdu(OLD));
                    world.settle().await;
                }
                // The absence starts; the station leaves during it.
                world.station_command(StaCommand::BackgroundScan).await;
                world.sent(&mut seen, NULL).await;
                world.station_command(StaCommand::Disconnect).await;
                world.sent(&mut seen, DEAUTHENTICATION).await;
                world
                    .notice(StaNotice::Disconnected(StaDisconnectReason::LocalRequest))
                    .await;
                // Reconnected: the arenas take the new link's MSDUs.
                world.associate(&mut seen, false).await;
                world.connected().await;
                let mut sent = 0;
                while world.send_msdu(NEW) {
                    sent += 1;
                    world.settle().await;
                }
                assert!(sent >= 3, "{sent}");
                world
                    .air
                    .hold_completion(WmmAccessCategory::BestEffort, false);
                for _ in 0..sent {
                    world.sent(&mut seen, DATA).await;
                }
                for _ in 0..5 {
                    world.settle().await;
                    world.advance();
                }
                let air = world.air.sent();
                let left = air
                    .iter()
                    .position(|frame| kind(frame) == DEAUTHENTICATION)
                    .unwrap();
                assert!(
                    !air[left..].iter().any(to(OLD)),
                    "no frame of the old link went after it left"
                );
                assert_eq!(air.iter().filter(|frame| to(NEW)(frame)).count(), sent);
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// Beside an access point on the upstream's channel, a background scan's
/// absence starts after one of its TBTTs; MSDUs the network sends during
/// the absence do not go while the station is away and all go once it is
/// back, none lost.
#[test]
fn beside_an_access_point_msdus_survive_an_absence() {
    pair(
        || two_channels(open()),
        std::vec![script(|world| {
            Box::pin(async move {
                use oer_portable_time::Clock as _;
                let mut seen = 0;
                world.associate(&mut seen, false).await;
                world.connected().await;
                let tbtt = world.clock.now().saturating_add(Duration::from_millis(5));
                world
                    .command(ControlCommand::AccessPoint(AccessPointFact::Running {
                        channel: config(StaSecurity::Open).0.channels[0],
                        next_tbtt: tbtt,
                        beacon_interval: crate::station::contract::BeaconInterval::new(
                            Duration::from_micros(102_400),
                        )
                        .unwrap(),
                    }))
                    .await;
                world.station_command(StaCommand::BackgroundScan).await;
                let away = world.sent(&mut seen, NULL).await;
                assert_ne!(away.plaintext[1] & 0x10, 0, "PM=1 before the absence");
                assert!(world.clock.now() >= tbtt, "the absence follows a TBTT");
                let left = seen;
                let destinations = [[0x02, 0, 0, 0, 0, 0x0c], [0x02, 0, 0, 0, 0, 0x0d]];
                for destination in destinations {
                    assert!(world.send_msdu(destination));
                }
                let back = world.sent(&mut seen, NULL).await;
                assert_eq!(back.plaintext[1] & 0x10, 0, "PM=0 once back");
                let returned = seen;
                assert!(
                    world.air.sent()[left..returned]
                        .iter()
                        .all(|frame| kind(frame) != DATA),
                    "no data while away"
                );
                for _ in destinations {
                    world.sent(&mut seen, DATA).await;
                }
                let air = world.air.sent();
                for destination in destinations {
                    assert_eq!(
                        air[returned..]
                            .iter()
                            .filter(|frame| kind(frame) == DATA
                                && frame.plaintext[16..22] == destination)
                            .count(),
                        1,
                        "{destination:02x?} went once"
                    );
                }
                world.stop().await;
            })
        })],
        |ends, _| assert!(stopped(ends)),
    );
}

/// An open station's leave, by the application's Disconnect or its
/// graceful stop, sends its Deauthentication on the association's control
/// record.
#[test]
fn an_open_station_s_leave_sends_its_deauthentication() {
    for graceful in [false, true] {
        pair(
            open,
            std::vec![script(move |world| {
                Box::pin(async move {
                    let mut seen = 0;
                    world.associate(&mut seen, false).await;
                    world.connected().await;
                    if graceful {
                        world.with_control(|control| {
                            control
                                .stop_gracefully(oer_portable_time::Instant::from_micros(u64::MAX))
                        });
                    } else {
                        world.station_command(StaCommand::Disconnect).await;
                    }
                    let deauthentication = world.sent(&mut seen, DEAUTHENTICATION).await;
                    assert_eq!(deauthentication.plaintext[4..10], BSSID);
                    assert_eq!(deauthentication.rate, dsss1());
                    assert!(world.heard.refusals.borrow().is_empty());
                    world.stop().await;
                })
            })],
            |ends, _| assert!(stopped(ends)),
        );
    }
}
