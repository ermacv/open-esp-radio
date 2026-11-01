//! The profile names the network of each connection: none before the first,
//! replaced only while idle, kept across reconnections.
use super::*;

fn open_profile(ssid: &[u8]) -> StaProfile {
    StaProfile {
        ssid: heapless::Vec::from_slice(ssid).unwrap(),
        security: StaSecurity::Open,
    }
}

fn wpa2_profile(ssid: &[u8], pmk: [u8; 32]) -> StaProfile {
    StaProfile {
        ssid: heapless::Vec::from_slice(ssid).unwrap(),
        security: StaSecurity::Wpa2Personal {
            pmk: Pmk::from_bytes(pmk),
        },
    }
}

/// A station built without a profile.
fn unprofiled(mint: Mint<'_, TestPools>) -> World<'_, 2> {
    World::with_builder::<2, 2>(mint, 3, [Slot(1), Slot(2)], build_station)
}

/// A scan of the open network's beacon and its outcome: the notice.
fn scan_beacon(world: &mut World<'_, 2>, now: u32) -> Vec<Seen> {
    world.command(at(now), 9, StaCommand::Connect).unwrap();
    world.run(at(now));
    world.advance();
    world.receive(at(now + 13), &beacon());
    world.advance();
    world.take()
}

#[test]
fn a_station_without_a_profile_refuses_connect_and_keeps_its_credit() {
    installed(|mint| {
        let mut world = unprofiled(mint);
        let refused = world.command(at(0), 1, StaCommand::Connect).unwrap_err();
        assert_eq!(refused.reason, CommandRefusal::NoProfile);
        assert!(world.command_credits.connect.is_some());
        assert!(
            world
                .station
                .poll_action(at(0), &mut world.losses)
                .is_none()
        );
        assert_eq!(world.station.policy(), None);
    });
}

#[test]
fn a_profile_is_refused_while_connecting_or_connected_and_comes_back_whole() {
    installed(|mint| {
        let mut world = World::new(mint, 3);
        world.command(at(0), 1, StaCommand::Connect).unwrap();
        world.run(at(0));
        let refused = world
            .station
            .set_profile(wpa2_profile(b"other", [7; 32]))
            .unwrap_err();
        assert_eq!(refused.reason, CommandRefusal::Transitioning);
        assert_eq!(refused.profile.ssid.as_slice(), b"other");
        assert!(matches!(
            &refused.profile.security,
            StaSecurity::Wpa2Personal { .. }
        ));
        world.advance();
        world.receive(at(13), &beacon());
        world.advance();
        world.answer_authentication(at(30), 0);
        world.receive(at(31), &open_association_response());
        assert!(world.station.is_connected());
        let refused = world
            .station
            .set_profile(open_profile(b"other"))
            .unwrap_err();
        assert_eq!(refused.reason, CommandRefusal::AlreadyConnected);
        // The station still joins the network it connected to.
        assert_eq!(world.station.policy(), Some(StaSecurityPolicy::Open));
        assert_eq!(world.station.transition().0.ssid(), SSID);
    });
}

#[test]
fn after_a_disconnect_a_new_profile_names_the_next_network() {
    installed(|mint| {
        let mut world = connect_open_world(World::new(mint, 3));
        world.command(at(40), 2, StaCommand::Disconnect).unwrap();
        world.run(at(40));
        assert!(matches!(world.station.state, State::Idle));
        world
            .station
            .set_profile(wpa2_profile(b"protected", [3; 32]))
            .unwrap();
        assert_eq!(world.station.policy(), Some(StaSecurityPolicy::Wpa2Psk));
        assert_eq!(world.station.transition().0.ssid(), b"protected");
        // The open network's beacon no longer names the profile's network.
        let seen = scan_beacon(&mut world, 100);
        assert!(
            seen.iter().any(|event| matches!(
                event,
                Seen::Notify(StaNotice::Disconnected(
                    StaDisconnectReason::NotFound { .. }
                ))
            )),
            "{seen:?}"
        );
        // Back to the open network: the same beacon starts the join.
        world.station.set_profile(open_profile(SSID)).unwrap();
        let seen = scan_beacon(&mut world, 400);
        assert!(
            !seen.iter().any(|event| matches!(event, Seen::Notify(_))),
            "{seen:?}"
        );
        assert!(matches!(world.station.state, State::Join(_)));
    });
}

#[test]
fn a_connection_ended_by_disconnect_connects_again_with_the_kept_profile() {
    installed(|mint| {
        let mut world = connect_open_world(World::new(mint, 3));
        world.command(at(40), 2, StaCommand::Disconnect).unwrap();
        world.run(at(40));
        assert!(matches!(world.station.state, State::Idle));
        world.command(at(50), 3, StaCommand::Connect).unwrap();
        world.run(at(50));
        assert!(matches!(world.station.state, State::Scan(_)));
        assert_eq!(world.station.transition().0.ssid(), SSID);
    });
}

#[test]
fn a_new_profile_forgets_the_previous_networks_cached_associations() {
    installed(|mint| {
        let mut world = unprofiled(mint);
        world.station.set_profile(open_profile(b"first")).unwrap();
        let bssid = |last| {
            oer_ieee80211_wire::address::Bssid::new(
                oer_ieee80211_wire::address::IndividualAddress::new([2, 0, 0, 0, 0, last]).unwrap(),
            )
            .unwrap()
        };
        for (last, ssid) in [(1, b"first".as_slice()), (2, b"other".as_slice())] {
            world.station.pmksa.insert(Pmksa::new(
                bssid(last),
                heapless::Vec::from_slice(ssid).unwrap(),
                Pmk::from_bytes([last; 32]),
                [last; 16],
            ));
        }
        world.station.set_profile(open_profile(b"second")).unwrap();
        assert!(world.station.pmksa.find(bssid(1), [1; 16]).is_none());
        assert!(world.station.pmksa.find(bssid(2), [2; 16]).is_some());
    });
}
