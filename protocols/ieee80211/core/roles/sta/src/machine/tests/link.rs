//! The station's link with its holder: handover, holds, numbering, EAPOL
//! routing, group replay lanes and the return.

use super::*;

/// The Sequence Number of a three-address frame the station sent.
fn sequence_of(frame: &[u8]) -> u16 {
    u16::from_le_bytes([frame[22], frame[23]]) >> 4
}

#[test]
fn a_connected_station_hands_its_link_over_once_and_releases_its_data() {
    installed(|mint| {
        let world = connected_world(mint);
        assert_eq!(world.link_actions, [LinkSeen::HandOver, LinkSeen::Release]);
        let link = world.link.as_ref().expect("the holder keeps the link");
        assert!(link.carries());
        assert!(!link.numbers().qos(), "the open test BSS is not a QoS BSS");
        assert!(link.numbers().key().is_none());
        // The station keeps no number of its own while the link is lent.
        assert!(world.station.link.lent());
    });
}

#[test]
fn a_protected_link_is_held_until_its_keys_are_confirmed() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_associated::<2>(mint);
        world.hold_transmissions = true;
        world.receive(at(33), &retry_message3(&ptk, 2, 0x99));
        // Connected, Message 4 in flight: the link is the holder's, held.
        assert!(!world.station.is_connected());
        assert_eq!(world.link_actions, [LinkSeen::HandOver]);
        let link = world.link.as_ref().unwrap();
        assert!(!link.carries());
        assert!(link.numbers().key().is_some(), "the link took the PTK");
        assert!(matches!(world.station.keys.pairwise, KeyCell::Lent(_)));
        world.hold_transmissions = false;
        world.end_transmissions(at(34));
        world.run(at(34));
        assert!(world.station.is_connected());
        assert_eq!(world.link_actions, [LinkSeen::HandOver, LinkSeen::Release]);
    });
}

#[test]
fn station_frames_and_data_draw_one_packet_number_space_of_the_link() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        // Data draws from the link's key as the plane's intake does.
        let data = |world: &mut World<'_>| {
            world
                .link
                .as_ref()
                .unwrap()
                .numbers()
                .key()
                .unwrap()
                .packet_numbers()
                .allocate()
                .unwrap()
                .value()
        };
        let before = data(&mut world);
        let sent = world.packet_numbers.len();
        world.receive_verified(at(50), &group_message1(&ptk, 3, 2));
        // Group Message 2 went under the link's key.
        assert_eq!(world.packet_numbers.len(), sent + 1);
        let response = *world.packet_numbers.last().unwrap();
        let after = data(&mut world);
        assert!(
            before < response && response < after,
            "{before} {response} {after}"
        );
    });
}

#[test]
fn eapol_of_the_link_reaches_the_station_only_through_the_link() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        let frame = group_message1(&ptk, 3, 2);
        // The station admits no connected data itself.
        assert_eq!(
            world
                .station
                .receive(at(50), &frame, verified_meta(), &mut world.losses),
            Err(oer_protocols_ieee80211_core_upper_mac::receive::RxDropReason::NotConnected.into())
        );
        assert!(world.poll(at(50)).is_none());
        // The link admits it and hands its EAPOL to the station, which owes
        // Group Message 2 under the link's key.
        assert_eq!(
            world.deliver(at(50), &frame, verified_meta()),
            Ok(StaReceived::Input)
        );
        world.run(at(50));
        assert_eq!(
            world.eapol.last(),
            Some(&(true, true)),
            "Group Message 2 under the pairwise key"
        );
    });
}

#[test]
fn the_returned_link_numbers_the_stations_frames_on_and_its_key_comes_back() {
    installed(|mint| {
        let (mut world, _) = wpa2_connected::<2>(mint);
        // Data took five numbers of the station's own space: the test BSS
        // is a non-QoS BSS.
        let link = world.link.as_mut().unwrap();
        assert!(!link.numbers().qos());
        for _ in 0..5 {
            link.numbers_mut()
                .data_and_key(oer_protocols_ieee80211_wire::qos::WmmUserPriority::UP0)
                .0
                .take();
        }
        let next = link
            .numbers()
            .next(oer_protocols_ieee80211_wire::qos::WmmUserPriority::UP0)
            .get();
        let sent = world.frames.len();
        world.command(at(50), 2, StaCommand::Disconnect).unwrap();
        world.run(at(50));
        // The station asked for its link back before its Deauthentication,
        // and for the key the holder kept only after it.
        assert_eq!(
            world.link_actions[world.link_actions.len() - 2..],
            [LinkSeen::Return, LinkSeen::ReturnKey]
        );
        assert!(world.link.is_none());
        assert!(world.kept_key.is_none());
        assert!(!world.station.link.lent());
        let deauthentication = world.frames[sent..]
            .iter()
            .find(|frame| frame[0] == 0xc0)
            .expect("the station deauthenticated");
        assert_eq!(sequence_of(deauthentication), next);
        // The pairwise key came back whole and left with the association.
        assert!(world.station.keys.clear());
        assert!(world.seen.contains(&Seen::RemoveKey));
        // A new connection numbers on from there.
        world.take();
        let sent = world.frames.len();
        world.command(at(60), 3, StaCommand::Connect).unwrap();
        world.run(at(60));
        world.advance();
        let probe = world.frames[sent..]
            .iter()
            .find(|frame| frame[0] == 0x40)
            .expect("the scan probes");
        assert_eq!(sequence_of(probe), next + 1);
    });
}

#[test]
fn only_the_terminal_through_the_link_switches_a_group_keys_replay_lane() {
    use {
        oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpRxProtection,
        oer_protocols_ieee80211_wire::ccmp::{CcmpHeader, CcmpPacketNumber},
    };
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        let header = |pn| {
            CcmpHeader::new(
                CcmpPacketNumber::new(pn).unwrap(),
                GroupKeyId::new(2).unwrap().key_id(),
            )
        };
        world.hold_key_installs = true;
        world.receive_verified(at(50), &group_message1(&ptk, 3, 2));
        assert_eq!(world.pending_installs.len(), 1);
        // The station holds no replay check of its own while it lends it.
        let State::Connected(connected) = &world.station.state else {
            panic!("connected")
        };
        assert!(connected.rsn.as_ref().unwrap().replay.is_none());
        // Before its terminal, key 2 has no lane: its frames are refused.
        assert!(
            world
                .replay()
                .check(CcmpRxProtection::Group, None, header(1))
                .is_err()
        );
        let input = world.pending_installs.pop().unwrap();
        world.input(at(51), input);
        world.run(at(51));
        // The terminal switched the lane in the link, from the key's RSC.
        world
            .replay()
            .check(CcmpRxProtection::Group, None, header(1))
            .unwrap();
        assert!(
            world
                .replay()
                .check(CcmpRxProtection::Group, None, header(1))
                .is_err()
        );
    });
}

#[test]
fn a_background_scan_holds_the_links_data_while_the_station_is_away() {
    installed(|mint| {
        let mut world = connected_world(mint);
        world.link_actions.clear();
        let now = at(40);
        world.command(now, 61, StaCommand::BackgroundScan).unwrap();
        world.run(now);
        for _ in 0..10 {
            if world.link_actions.contains(&LinkSeen::Release) {
                break;
            }
            world.advance();
        }
        assert_eq!(world.link_actions, [LinkSeen::Hold, LinkSeen::Release]);
        assert!(world.link.as_ref().unwrap().carries());
    });
}
