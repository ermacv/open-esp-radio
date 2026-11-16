//! A station asked for HE20: what it requests from an access point with and
//! without HE, and what it makes of the Association Response.

use super::*;
use crate::association::{StaHePower, StaPhyRequest};
use oer_protocols_ieee80211_wire::station::{HeUlMuPowerCapability, StaPowerCapability};

/// An HT Capabilities element of an HT20 access point on channel 6.
fn ht_capability() -> [u8; 28] {
    use oer_protocols_ieee80211_wire::ht::{HtLocalCapabilities, ht_capability_ie};
    ht_capability_ie(
        HtLocalCapabilities::new(0, 0, 0xff, 1),
        Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
    )
}

/// An HE Capabilities element with HE-MCS 0..9 both ways on one stream.
const HE_CAPABILITY: [u8; 24] = [
    255, 22, 35, 0x01, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x00, 0x00, 0x00,
    0x00, 0x02, 0x80, 0x01, 0xfd, 0xff, 0xfd, 0xff,
];
/// An HE Operation element: BSS color 27, default PE 4, ER-SU disabled.
const HE_OPERATION: [u8; 9] = [255, 7, 36, 0x04, 0x00, 0x01, 0x1b, 0xfc, 0xff];

/// The station's own elements, each distinct, so a request shows which
/// one it carries.
static ELEMENTS: AssociationCapabilities = AssociationCapabilities {
    ht: oer_protocols_ieee80211_wire::station::HtCapabilityElements {
        mhz20: ht20_element(0x11),
        mhz40: ht20_element(0x22),
    },
    ht_with_he: ht20_element(0x33),
    he: HE_CAPABILITY,
    he_extended: Some([127, 12, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    wmm: [221, 7, 0, 0x50, 0xf2, 2, 0, 1, 0],
};

/// An HT Capabilities element whose capability information is `marker`.
const fn ht20_element(marker: u8) -> [u8; 28] {
    let mut element = [0; 28];
    element[0] = 45;
    element[1] = 26;
    element[2] = marker;
    element
}

fn power() -> StaHePower {
    StaHePower {
        power_capability: StaPowerCapability::new(-11, 20).unwrap(),
        ul_mu_power: HeUlMuPowerCapability::from_rate_power_indices([
            20, 20, 20, 20, 19, 19, 18, 18, 16, 15,
        ])
        .unwrap(),
    }
}

/// The beacon of channel 6 with HT, and with HE when `he`.
fn access_point_beacon(he: bool) -> Vec<u8> {
    use oer_protocols_ieee80211_wire::ht::ht_operation_ie;
    let mut frame = with_element(&beacon(), &ht_capability());
    frame = with_element(
        &frame,
        &ht_operation_ie(
            Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(),
            Default::default(),
        ),
    );
    if he {
        frame = with_element(&frame, &HE_CAPABILITY);
        frame = with_element(&frame, &HE_OPERATION);
    }
    frame
}

/// The open network's successful Association Response, with HT
/// Capabilities, and with `he`'s HE elements.
fn response(he: &[&[u8]]) -> Vec<u8> {
    let mut frame = with_element(&open_association_response(), &ht_capability());
    for element in he {
        frame = with_element(&frame, element);
    }
    frame
}

/// Each element of an Association Request: its ID, and for an extension
/// element its Element ID Extension.
fn request_elements(frame: &[u8]) -> Vec<(u8, Option<u8>)> {
    let mut elements = Vec::new();
    let mut rest = &frame[28..];
    while let [id, length, tail @ ..] = rest {
        let (value, next) = tail.split_at(usize::from(*length));
        elements.push((*id, (*id == 255).then(|| value[0])));
        rest = next;
    }
    elements
}

/// The world asking for `request`, scanned, authenticated and its
/// Association Request sent to the access point of `beacon`.
fn associating<'id>(
    mint: Mint<'id, TestPools>,
    request: StaPhyRequest<Caps>,
    beacon: &[u8],
) -> (World<'id>, Vec<u8>) {
    let mut world = World::new(mint, 3);
    world.station.config.association = &ELEMENTS;
    world
        .station
        .profile
        .as_mut()
        .expect("the world's profile")
        .phy = request;
    let request = join(&mut world, 0, 1, beacon);
    (world, request)
}

/// Connect from idle at `start` ms with command tag `tag`, hear `beacon`
/// and authenticate: the Association Request sent.
fn join(world: &mut World<'_>, start: u32, tag: u64, beacon: &[u8]) -> Vec<u8> {
    let sent = world.frames.len();
    world.command(at(start), tag, StaCommand::Connect).unwrap();
    world.run(at(start));
    world.advance();
    world.receive(at(start + 13), beacon);
    world.advance();
    world.answer_authentication(at(start + 30), 0);
    let requests = world.frames[sent..]
        .iter()
        .filter(|frame| frame[0] & 0xfc == 0x00)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    requests.into_iter().next().unwrap()
}

fn connected_phy(
    world: &World<'_>,
) -> Option<oer_protocols_ieee80211_core_upper_mac::phy_mode::PhyMode> {
    world
        .completed
        .iter()
        .find_map(|terminal| match terminal.outcome {
            StaCommandOutcome::Connected { phy, .. } => Some(phy),
            _ => None,
        })
}

#[test]
fn an_he_access_point_gets_the_he_request_and_an_he_link() {
    installed(|mint| {
        let (mut world, request) = associating(
            mint,
            request(PHY_NON_HT_AND_HT.union(PHY_HE), Width::Mhz40),
            &access_point_beacon(true),
        );
        // The vendor's order: Power Capability after the rates, HT and HE
        // Capabilities, the UL-MU power, WMM, Extended Capabilities.
        assert_eq!(
            request_elements(&request),
            [
                (0, None),
                (1, None),
                (33, None),
                (45, None),
                (255, Some(35)),
                (255, Some(60)),
                (221, None),
                (127, None),
            ]
        );
        let body = &request[28..];
        let find = |needle: &[u8]| body.windows(needle.len()).any(|window| window == needle);
        assert!(find(&[33, 2, 0xf5, 20]), "Power Capability -11 / 20 dBm");
        assert!(find(&[255, 12, 60, 0, 0, 0, 1, 1, 2, 2, 4, 5, 0, 0]));
        assert!(find(&ELEMENTS.ht_with_he), "the HE mode's HT Capabilities");
        assert!(find(&HE_CAPABILITY));

        world.receive(at(31), &response(&[&HE_CAPABILITY, &HE_OPERATION]));
        assert!(world.station.is_connected());
        let peer = world.station.peer().unwrap();
        assert_eq!(
            peer.associated.phy,
            oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::He { power_capability: oer_protocols_ieee80211_wire::station::StaPowerCapability::new(0, 20).unwrap(), he_ul_mu_power: oer_protocols_ieee80211_wire::station::HeUlMuPowerCapability::from_rate_power_indices([20; 10]).unwrap() }.mode()
        );
        assert_eq!(peer.associated.he_bss_color, 27);
        assert!(
            peer.associated
                .he_peer_state
                .is_some_and(|state| state.extended_range_single_user_disabled())
        );
        assert!(world.take().iter().any(|seen| matches!(
            seen,
            Seen::Notify(StaNotice::Connected { phy, .. }) if *phy == oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::He { power_capability: oer_protocols_ieee80211_wire::station::StaPowerCapability::new(0, 20).unwrap(), he_ul_mu_power: oer_protocols_ieee80211_wire::station::HeUlMuPowerCapability::from_rate_power_indices([20; 10]).unwrap() }.mode()
        )));
        assert_eq!(
            connected_phy(&world),
            Some(oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::He { power_capability: oer_protocols_ieee80211_wire::station::StaPowerCapability::new(0, 20).unwrap(), he_ul_mu_power: oer_protocols_ieee80211_wire::station::HeUlMuPowerCapability::from_rate_power_indices([20; 10]).unwrap() }.mode())
        );
    });
}

#[test]
fn an_access_point_without_he_gets_the_ht_request() {
    installed(|mint| {
        let (mut world, request) = associating(
            mint,
            request(PHY_NON_HT_AND_HT.union(PHY_HE), Width::Mhz20),
            &access_point_beacon(false),
        );
        // HT20 without the HE elements or the HE path's Power Capability.
        assert_eq!(
            request_elements(&request),
            [(0, None), (1, None), (45, None), (221, None)]
        );
        assert!(request[28..].windows(28).any(|w| w == ELEMENTS.ht.mhz20));

        world.receive(at(31), &response(&[]));
        assert!(world.station.is_connected());
        assert_eq!(
            world.station.peer().unwrap().associated.phy,
            oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::Ht {
                width: oer_protocols_ieee80211_contracts_port::Width::Mhz20
            }
            .mode()
        );
        assert_eq!(
            connected_phy(&world),
            Some(
                oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::Ht {
                    width: oer_protocols_ieee80211_contracts_port::Width::Mhz20
                }
                .mode()
            )
        );
    });
}

#[test]
fn an_access_point_that_does_not_grant_he_is_no_link() {
    // An HE request answered without the HE Operation, or without the HE
    // Capabilities, is an HE setup the access point did not make: the
    // station leaves rather than keep an HT link.
    for granted in [&[&HE_CAPABILITY[..]][..], &[&HE_OPERATION[..]][..], &[][..]] {
        installed(|mint| {
            let (mut world, _) = associating(
                mint,
                request(PHY_NON_HT_AND_HT.union(PHY_HE), Width::Mhz20),
                &access_point_beacon(true),
            );
            world.receive(at(31), &response(granted));
            world.run(at(31));
            assert!(!world.station.is_connected());
            assert!(world.station.peer().is_none());
            assert_eq!(connected_phy(&world), None);
            assert!(
                !world
                    .take()
                    .iter()
                    .any(|seen| matches!(seen, Seen::Notify(StaNotice::Connected { .. })))
            );
            assert!(world.completed.iter().any(|terminal| matches!(
                terminal.outcome,
                StaCommandOutcome::ConnectFailed(StaDisconnectReason::HeNotGranted)
            )));
        });
    }
}

/// The station asked for HE20 from the HE access point and its response
/// carried `elements`: whether it connected, and its peer when it did.
fn answered(elements: &[&[u8]]) -> (bool, Option<StaAssociatedPeer>, Option<StaCommandOutcome>) {
    installed(|mint| {
        let (mut world, _) = associating(
            mint,
            request(PHY_NON_HT_AND_HT.union(PHY_HE), Width::Mhz20),
            &access_point_beacon(true),
        );
        world.receive(at(31), &response(elements));
        world.run(at(31));
        (
            world.station.is_connected(),
            world.station.peer().map(|peer| peer.associated),
            world.completed.last().map(|terminal| terminal.outcome),
        )
    })
}

fn refused_he(
    connected: bool,
    peer: Option<StaAssociatedPeer>,
    outcome: Option<StaCommandOutcome>,
) {
    assert!(!connected);
    assert!(peer.is_none());
    assert_eq!(
        outcome,
        Some(StaCommandOutcome::ConnectFailed(
            StaDisconnectReason::HeNotGranted
        ))
    );
}

#[test]
fn he_elements_without_bodies_are_no_he_link() {
    // The HE Element ID Extensions alone: presence is not an HE setup.
    let (connected, peer, outcome) = answered(&[&[255, 1, 35], &[255, 1, 36]]);
    refused_he(connected, peer, outcome);
}

#[test]
fn the_response_s_bss_color_is_the_link_s() {
    // The beacon announced BSS color 27; the association answers with 5.
    let mut operation = HE_OPERATION;
    operation[6] = 5;
    let (connected, peer, _) = answered(&[&HE_CAPABILITY, &operation]);
    assert!(connected);
    let peer = peer.unwrap();
    assert_eq!(
        peer.phy,
        oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::He { power_capability: oer_protocols_ieee80211_wire::station::StaPowerCapability::new(0, 20).unwrap(), he_ul_mu_power: oer_protocols_ieee80211_wire::station::HeUlMuPowerCapability::from_rate_power_indices([20; 10]).unwrap() }.mode()
    );
    assert_eq!(peer.he_bss_color, 5);
    assert_eq!(peer.he_peer_state.unwrap().bss_color(), 5);
    // Its other HE Operation parameters are the response's too: default PE
    // 2 and ER-SU permitted where the beacon said 4 and disabled.
    let mut operation = HE_OPERATION;
    operation[3] = 0x02;
    operation[5] = 0x00;
    let (_, peer, _) = answered(&[&HE_CAPABILITY, &operation]);
    let state = peer.unwrap().he_peer_state.unwrap();
    assert_eq!(state.default_packet_extension_duration(), 2);
    assert!(state.extended_range_single_user_permitted());
}

#[test]
fn a_response_that_no_longer_admits_he20_is_refused() {
    // HE-MCS 0..7 only: not the HE20 the scan admitted.
    let mut capability = HE_CAPABILITY;
    capability[20..24].copy_from_slice(&[0xfc, 0xff, 0xfc, 0xff]);
    let (connected, peer, outcome) = answered(&[&capability, &HE_OPERATION]);
    refused_he(connected, peer, outcome);
}

#[test]
fn malformed_or_repeated_he_elements_are_refused() {
    let short_operation: &[u8] = &[255, 4, 36, 0, 0, 0];
    for elements in [
        &[&HE_CAPABILITY[..], short_operation][..],
        &[&HE_CAPABILITY[..], &HE_OPERATION[..], &HE_OPERATION[..]][..],
        &[&HE_CAPABILITY[..], &HE_CAPABILITY[..], &HE_OPERATION[..]][..],
    ] {
        let (connected, peer, outcome) = answered(elements);
        refused_he(connected, peer, outcome);
    }
}

#[test]
fn a_station_not_asking_for_he_never_requests_it() {
    installed(|mint| {
        let (mut world, request) = associating(
            mint,
            request(PHY_NON_HT_AND_HT, Width::Mhz20),
            &access_point_beacon(true),
        );
        assert_eq!(
            request_elements(&request),
            [(0, None), (1, None), (45, None), (221, None)]
        );
        // The access point's HE elements in its response make no HE link.
        world.receive(at(31), &response(&[&HE_CAPABILITY, &HE_OPERATION]));
        assert!(world.station.is_connected());
        assert_eq!(
            world.station.peer().unwrap().associated.phy,
            oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::Ht {
                width: oer_protocols_ieee80211_contracts_port::Width::Mhz20
            }
            .mode()
        );
        assert_eq!(
            connected_phy(&world),
            Some(
                oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::Ht {
                    width: oer_protocols_ieee80211_contracts_port::Width::Mhz20
                }
                .mode()
            )
        );
    });
}

#[test]
fn he_without_ht_is_not_admitted() {
    // An access point advertising HE elements but no HT Capabilities does
    // not admit HE20: the request falls to the HT40 rules, which make a
    // non-HT association with it.
    installed(|mint| {
        let mut beacon = beacon().to_vec();
        beacon = with_element(&beacon, &HE_CAPABILITY);
        beacon = with_element(&beacon, &HE_OPERATION);
        let (mut world, request) = associating(
            mint,
            request(PHY_NON_HT_AND_HT.union(PHY_HE), Width::Mhz40),
            &beacon,
        );
        assert_eq!(request_elements(&request), [(0, None), (1, None)]);
        world.receive(at(31), &open_association_response());
        assert!(world.station.is_connected());
        assert_eq!(
            connected_phy(&world),
            Some(oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::NonHt.mode())
        );
    });
}

/// The HE states the station set, in order of the actions it took.
fn he_settings(seen: &[Seen]) -> Vec<oer_protocols_ieee80211_contracts_port::StationHe> {
    seen.iter()
        .filter_map(|seen| match seen {
            Seen::He(he) => Some(*he),
            _ => None,
        })
        .collect()
}

/// The position of the first `seen` that `pick` names.
fn position(seen: &[Seen], pick: impl Fn(&Seen) -> bool) -> Option<usize> {
    seen.iter().position(pick)
}

/// The HE BSS the beacon's HE Operation names: color 27, enabled.
fn scanned_bss() -> oer_protocols_ieee80211_contracts_port::StationHe {
    oer_protocols_ieee80211_contracts_port::StationHe::Bss(
        oer_protocols_ieee80211_contracts_port::HeBssColor::of(
            &oer_protocols_ieee80211_wire::he::parse_he_operation(&HE_OPERATION).unwrap(),
        )
        .unwrap(),
    )
}

/// The HE request the joins below ask for.
fn prefer() -> StaPhyRequest<Caps> {
    request(PHY_NON_HT_AND_HT.union(PHY_HE), Width::Mhz20)
}

#[test]
fn a_profile_whose_request_admits_no_he_gets_the_ht_request() {
    installed(|mint| {
        let mut world = World::new(mint, 3);
        world.station.config.association = &ELEMENTS;
        world
            .station
            .profile
            .as_mut()
            .expect("the world's profile")
            .phy = request(PHY_NON_HT_AND_HT, Width::Mhz20);
        let request = join(&mut world, 0, 1, &access_point_beacon(true));
        assert_eq!(
            request_elements(&request),
            [(0, None), (1, None), (45, None), (221, None)]
        );
        world.receive(at(31), &response(&[&HE_CAPABILITY, &HE_OPERATION]));
        assert_eq!(
            connected_phy(&world),
            Some(
                oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::Ht {
                    width: oer_protocols_ieee80211_contracts_port::Width::Mhz20
                }
                .mode()
            )
        );
        // An HT join starts with the port's HE off, as the vendor's
        // non-HE connect does, and sets nothing more.
        assert_eq!(
            he_settings(&world.take()),
            [oer_protocols_ieee80211_contracts_port::StationHe::Off]
        );
    });
}

#[test]
fn an_he_join_sets_its_bss_before_authentication_and_its_association_before_its_data() {
    use oer_protocols_ieee80211_contracts_port::StationHe;
    installed(|mint| {
        let (mut world, _) = associating(mint, prefer(), &access_point_beacon(true));
        world.receive(at(31), &response(&[&HE_CAPABILITY, &HE_OPERATION]));
        let seen = world.take();
        // The BSS, with the scan's color, before the receive configuration
        // and the Authentication; the association after the EDCA set and
        // before the connection.
        let bss = position(&seen, |seen| *seen == Seen::He(scanned_bss()));
        let configure = position(&seen, |seen| *seen == Seen::Configure("bss"));
        let authentication = position(&seen, |seen| {
            matches!(
                seen,
                Seen::Transmit {
                    subtype: 0x00b0,
                    ..
                }
            )
        });
        let edca = position(&seen, |seen| matches!(seen, Seen::Edca(_)));
        let association = position(&seen, |seen| matches!(seen, Seen::He(StationHe::He(_))));
        let connected = position(&seen, |seen| {
            matches!(seen, Seen::Notify(StaNotice::Connected { .. }))
        });
        assert!(
            bss.is_some() && bss < configure && configure < authentication,
            "{seen:?}"
        );
        assert!(
            authentication < edca && edca < association && association < connected,
            "{seen:?}"
        );
        let settings = he_settings(&seen);
        assert_eq!(settings.len(), 2, "{seen:?}");
        let StationHe::He(association) = settings[1] else {
            panic!("the association: {seen:?}");
        };
        // The response's AID (0xc02a less its two top bits), the access
        // point's minimum MPDU start spacing (none) and its HE Operation:
        // default PE 4, ER-SU disabled, no co-hosted BSS.
        assert_eq!(association.association_id(), 0x2a);
        assert_eq!(association.minimum_mpdu_start_spacing(), 0);
        assert_eq!(association.default_packet_extension(), 4);
        assert_eq!(association.max_co_hosted_bssid_indicator(), None);
        assert!(association.extended_range_single_user_disabled());

        // The leave ends it before receive goes off.
        world.command(at(40), 2, StaCommand::Disconnect).unwrap();
        world.run(at(40));
        let seen = world.take();
        let off = position(&seen, |seen| *seen == Seen::He(StationHe::Off));
        let configure = position(&seen, |seen| *seen == Seen::Configure("off"));
        assert!(off.is_some() && off < configure, "{seen:?}");
        assert_eq!(he_settings(&seen), [StationHe::Off]);
    });
}

#[test]
fn an_ht_link_starts_with_he_off_and_sets_nothing_more() {
    use oer_protocols_ieee80211_contracts_port::StationHe;
    installed(|mint| {
        let (mut world, _) = associating(mint, prefer(), &access_point_beacon(false));
        world.receive(at(31), &response(&[]));
        world.command(at(40), 2, StaCommand::Disconnect).unwrap();
        world.run(at(40));
        assert_eq!(he_settings(&world.take()), [StationHe::Off]);
    });
}

#[test]
fn he_then_ht_then_he_sets_each_bss_and_association_and_ends_each() {
    use oer_protocols_ieee80211_contracts_port::StationHe;
    installed(|mint| {
        let (mut world, _) = associating(mint, prefer(), &access_point_beacon(true));
        world.receive(at(31), &response(&[&HE_CAPABILITY, &HE_OPERATION]));
        world.command(at(40), 2, StaCommand::Disconnect).unwrap();
        world.run(at(40));
        let first = he_settings(&world.take());
        assert!(
            matches!(
                first[..],
                [StationHe::Bss(_), StationHe::He(_), StationHe::Off]
            ),
            "{first:?}"
        );

        // The access point drops HE: the next link is HT and its join
        // starts with HE off.
        let request = join(&mut world, 100, 3, &access_point_beacon(false));
        assert!(!request_elements(&request).contains(&(255, Some(35))));
        world.receive(at(131), &response(&[]));
        assert_eq!(
            world.station.peer().unwrap().associated.phy,
            oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::Ht {
                width: oer_protocols_ieee80211_contracts_port::Width::Mhz20
            }
            .mode()
        );
        world.command(at(140), 4, StaCommand::Disconnect).unwrap();
        world.run(at(140));
        assert_eq!(he_settings(&world.take()), [StationHe::Off]);

        // HE again: the BSS and the association are set once more.
        let request = join(&mut world, 200, 5, &access_point_beacon(true));
        assert!(request_elements(&request).contains(&(255, Some(35))));
        world.receive(at(231), &response(&[&HE_CAPABILITY, &HE_OPERATION]));
        assert_eq!(
            world.station.peer().unwrap().associated.phy,
            oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::He { power_capability: oer_protocols_ieee80211_wire::station::StaPowerCapability::new(0, 20).unwrap(), he_ul_mu_power: oer_protocols_ieee80211_wire::station::HeUlMuPowerCapability::from_rate_power_indices([20; 10]).unwrap() }.mode()
        );
        let again = he_settings(&world.take());
        assert!(
            matches!(again[..], [StationHe::Bss(_), StationHe::He(_)]),
            "{again:?}"
        );
    });
}

#[test]
fn a_refused_association_between_the_stages_ends_the_bss_before_the_next_ht_join() {
    use oer_protocols_ieee80211_contracts_port::StationHe;
    // The BSS is set, then the access point refuses the association
    // (status 17): the leave ends the BSS; the next join of an access point
    // without HE starts with HE off and no association is ever set.
    installed(|mint| {
        let (mut world, _) = associating(mint, prefer(), &access_point_beacon(true));
        let mut refused = response(&[&HE_CAPABILITY, &HE_OPERATION]);
        refused[26..28].copy_from_slice(&17_u16.to_le_bytes());
        world.receive(at(31), &refused);
        world.run(at(31));
        assert!(!world.station.is_connected());
        let seen = world.take();
        assert_eq!(
            he_settings(&seen),
            [scanned_bss(), StationHe::Off],
            "{seen:?}"
        );
        let off = position(&seen, |seen| *seen == Seen::He(StationHe::Off));
        let configure = position(&seen, |seen| *seen == Seen::Configure("off"));
        assert!(off < configure, "{seen:?}");

        let request = join(&mut world, 100, 3, &access_point_beacon(false));
        assert!(!request_elements(&request).contains(&(255, Some(35))));
        world.receive(at(131), &response(&[]));
        assert_eq!(
            world.station.peer().unwrap().associated.phy,
            oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::Ht {
                width: oer_protocols_ieee80211_contracts_port::Width::Mhz20
            }
            .mode()
        );
        assert_eq!(he_settings(&world.take()), [StationHe::Off]);
    });
}

#[test]
fn an_he_response_with_an_aid_outside_its_field_is_no_link() {
    use oer_protocols_ieee80211_contracts_port::StationHe;
    installed(|mint| {
        let (mut world, _) = associating(mint, prefer(), &access_point_beacon(true));
        let mut frame = response(&[&HE_CAPABILITY, &HE_OPERATION]);
        // AID 0, its two top bits set.
        frame[28..30].copy_from_slice(&0xc000_u16.to_le_bytes());
        world.receive(at(31), &frame);
        world.run(at(31));
        assert!(!world.station.is_connected());
        // The BSS set before authentication ends; no association is set.
        assert_eq!(he_settings(&world.take()), [scanned_bss(), StationHe::Off]);
        assert!(world.completed.iter().any(|terminal| matches!(
            terminal.outcome,
            StaCommandOutcome::ConnectFailed(StaDisconnectReason::HeNotGranted)
        )));
    });
}

#[test]
fn the_association_carries_the_response_s_color_over_the_scan_s() {
    use oer_protocols_ieee80211_contracts_port::{BssColor, StationHe};
    // The owner's counterexample: the beacon names color 27, the
    // Association Response 5. The BSS goes with the scan's 27 before
    // authentication; the association carries the response's 5.
    installed(|mint| {
        let (mut world, _) = associating(mint, prefer(), &access_point_beacon(true));
        let mut operation = HE_OPERATION;
        operation[6] = 5;
        world.receive(at(31), &response(&[&HE_CAPABILITY, &operation]));
        let settings = he_settings(&world.take());
        let [StationHe::Bss(scanned), StationHe::He(association)] = settings[..] else {
            panic!("the BSS and the association: {settings:?}");
        };
        assert_eq!(scanned.get().0, BssColor::new(27).unwrap());
        assert_eq!(
            association.color().get(),
            (
                BssColor::new(5).unwrap(),
                oer_protocols_ieee80211_contracts_port::BssColorUse::Enabled,
                oer_protocols_ieee80211_contracts_port::BssColorScope::Full
            )
        );
        assert_eq!(world.station.peer().unwrap().associated.he_bss_color, 5);
    });
}

#[test]
fn a_restarted_station_sets_no_he_off_for_an_association_of_its_last_run() {
    installed(|mint| {
        let (mut world, _) = associating(mint, prefer(), &access_point_beacon(true));
        world.receive(at(31), &response(&[&HE_CAPABILITY, &HE_OPERATION]));
        assert_eq!(
            world.station.peer().unwrap().associated.phy,
            oer_protocols_ieee80211_core_upper_mac::phy_mode::NegotiatedPhy::He { power_capability: oer_protocols_ieee80211_wire::station::StaPowerCapability::new(0, 20).unwrap(), he_ul_mu_power: oer_protocols_ieee80211_wire::station::HeUlMuPowerCapability::from_rate_power_indices([20; 10]).unwrap() }.mode()
        );
        let mut world = restarted(world);
        world.take();
        world.run(at(50));
        let seen = world.take();
        assert!(he_settings(&seen).is_empty(), "{seen:?}");
        assert!(seen.contains(&Seen::Configure("off")));
    });
}

/// The negotiation of 088's table, every cell: the request's formats as a
/// set and its widest width against five access points (none with HT; HT
/// without a usable secondary; HT with one; HE on each of those two),
/// the format first and then the width.
#[test]
fn the_negotiation_takes_the_admitted_format_first_then_the_width() {
    let scanned = wide(13, ChannelWidth::Mhz20);
    let below = wide(13, ChannelWidth::Mhz40Below);
    let he = |mut record: ScanRecord| {
        record.set_he_capability_ie(&HE_CAPABILITY).unwrap();
        record.set_he_operation_ie(&HE_OPERATION).unwrap();
        record
    };
    let n = ht_access_point(None);
    let h = ht_access_point(Some(ChannelWidth::Mhz20));
    let w = ht_access_point(Some(ChannelWidth::Mhz40Below));
    let eh = he(h.clone());
    let ew = he(w.clone());
    let non_ht = Some((scanned, NegotiatedPhy::NonHt));
    let ht20 = Some((
        scanned,
        NegotiatedPhy::Ht {
            width: Width::Mhz20,
        },
    ));
    let ht40 = Some((
        below,
        NegotiatedPhy::Ht {
            width: Width::Mhz40,
        },
    ));
    let he20 = Some((
        scanned,
        NegotiatedPhy::He {
            power_capability: power().power_capability,
            he_ul_mu_power: power().ul_mu_power,
        },
    ));
    let rows: [(
        StaPhyRequest<WideCaps>,
        [Option<(SupportedChannel<WideCaps>, NegotiatedPhy)>; 5],
    ); 8] = [
        (
            request(PHY_NON_HT, Width::Mhz20),
            [non_ht, non_ht, non_ht, non_ht, non_ht],
        ),
        (
            request(PHY_NON_HT_AND_HT, Width::Mhz20),
            [non_ht, ht20, ht20, ht20, ht20],
        ),
        (
            request(PHY_NON_HT_AND_HT, Width::Mhz40),
            [non_ht, ht20, ht40, ht20, ht40],
        ),
        (
            request(PHY_NON_HT_AND_HT.union(PHY_HE), Width::Mhz20),
            [non_ht, ht20, ht20, he20, he20],
        ),
        (
            request(PHY_NON_HT_AND_HT.union(PHY_HE), Width::Mhz40),
            [non_ht, ht20, ht40, he20, he20],
        ),
        (
            request(PHY_NONE.union(PHY_HE), Width::Mhz20),
            [None, None, None, he20, he20],
        ),
        (
            request(PHY_HT.union(PHY_HE), Width::Mhz40),
            [None, ht20, ht40, he20, he20],
        ),
        (
            request(PHY_HT.union(PHY_HE), Width::Mhz20),
            [None, ht20, ht20, he20, he20],
        ),
    ];
    for (row, (asked, expected)) in rows.iter().enumerate() {
        for (column, (access_point, expected)) in
            [&n, &h, &w, &eh, &ew].into_iter().zip(expected).enumerate()
        {
            assert_eq!(
                negotiate(asked, power(), access_point, &[below], scanned),
                *expected,
                "row {row}, access point {column}"
            );
        }
    }
}

/// A request of HE alone refuses an access point without HE when the
/// candidate is chosen: no Authentication and no Association Request goes
/// to it, and the join ends with its own reason.
#[test]
fn an_he_only_request_refuses_an_access_point_without_he_before_any_frame() {
    installed(|mint| {
        let mut world = World::new(mint, 3);
        world.station.config.association = &ELEMENTS;
        world
            .station
            .profile
            .as_mut()
            .expect("the world's profile")
            .phy = request(PHY_NONE.union(PHY_HE), Width::Mhz20);
        let sent = world.frames.len();
        world.command(at(0), 1, StaCommand::Connect).unwrap();
        world.run(at(0));
        world.advance();
        world.receive(at(13), &access_point_beacon(false));
        let refused = |world: &World<'_>| {
            world.seen.iter().any(|seen| {
                matches!(
                    seen,
                    Seen::Notify(StaNotice::Disconnected(
                        StaDisconnectReason::NoAdmittedFormat { .. }
                    ))
                )
            })
        };
        for _ in 0..16 {
            if refused(&world) {
                break;
            }
            world.advance();
        }
        assert!(refused(&world), "{:?}", world.seen);
        assert!(
            world.frames[sent..]
                .iter()
                .all(|frame| frame[0] & 0xfc != 0xb0 && frame[0] & 0xfc != 0x00),
            "no Authentication and no Association Request"
        );
    });
}
