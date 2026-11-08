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
    ht20: ht20_element(0x11),
    ht40: ht20_element(0x22),
    he20_ht: ht20_element(0x33),
    he20: HE_CAPABILITY,
    he20_extended: [127, 12, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
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
    request: StaPhyRequest,
    beacon: &[u8],
) -> (World<'id>, Vec<u8>) {
    let mut world = World::new(mint, 3);
    world.station.config.association = &ELEMENTS;
    world.station.config.phy = request;
    world.command(at(0), 1, StaCommand::Connect).unwrap();
    world.run(at(0));
    world.advance();
    world.receive(at(13), beacon);
    world.advance();
    world.answer_authentication(at(30), 0);
    let requests = world
        .frames
        .iter()
        .filter(|frame| frame[0] & 0xfc == 0x00)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    let request = requests.into_iter().next().unwrap();
    (world, request)
}

fn connected_phy(world: &World<'_>) -> Option<PhyMode> {
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
            StaPhyRequest::PreferHe20 {
                ht40: true,
                power: power(),
            },
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
        assert!(find(&ELEMENTS.he20_ht), "the HE mode's HT Capabilities");
        assert!(find(&HE_CAPABILITY));

        world.receive(at(31), &response(&[&HE_CAPABILITY, &HE_OPERATION]));
        assert!(world.station.is_connected());
        let peer = world.station.peer().unwrap();
        assert_eq!(peer.associated.phy, PhyMode::He20);
        assert_eq!(peer.associated.he_bss_color, 27);
        assert!(
            peer.associated
                .he_peer_state
                .is_some_and(|state| state.extended_range_single_user_disabled)
        );
        assert!(world.take().iter().any(|seen| matches!(
            seen,
            Seen::Notify(StaNotice::Connected {
                phy: PhyMode::He20,
                ..
            })
        )));
        assert_eq!(connected_phy(&world), Some(PhyMode::He20));
    });
}

#[test]
fn an_access_point_without_he_gets_the_ht_request() {
    installed(|mint| {
        let (mut world, request) = associating(
            mint,
            StaPhyRequest::PreferHe20 {
                ht40: false,
                power: power(),
            },
            &access_point_beacon(false),
        );
        // HT20 without the HE elements or the HE path's Power Capability.
        assert_eq!(
            request_elements(&request),
            [(0, None), (1, None), (45, None), (221, None)]
        );
        assert!(request[28..].windows(28).any(|w| w == ELEMENTS.ht20));

        world.receive(at(31), &response(&[]));
        assert!(world.station.is_connected());
        assert_eq!(world.station.peer().unwrap().associated.phy, PhyMode::Ht20);
        assert_eq!(connected_phy(&world), Some(PhyMode::Ht20));
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
                StaPhyRequest::PreferHe20 {
                    ht40: false,
                    power: power(),
                },
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
            StaPhyRequest::PreferHe20 {
                ht40: false,
                power: power(),
            },
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
    assert_eq!(peer.phy, PhyMode::He20);
    assert_eq!(peer.he_bss_color, 5);
    assert_eq!(peer.he_peer_state.unwrap().bss_color, 5);
    // Its other HE Operation parameters are the response's too: default PE
    // 2 and ER-SU permitted where the beacon said 4 and disabled.
    let mut operation = HE_OPERATION;
    operation[3] = 0x02;
    operation[5] = 0x00;
    let (_, peer, _) = answered(&[&HE_CAPABILITY, &operation]);
    let state = peer.unwrap().he_peer_state.unwrap();
    assert_eq!(state.default_packet_extension_duration, 2);
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
        let (mut world, request) =
            associating(mint, StaPhyRequest::Ht20, &access_point_beacon(true));
        assert_eq!(
            request_elements(&request),
            [(0, None), (1, None), (45, None), (221, None)]
        );
        // The access point's HE elements in its response make no HE link.
        world.receive(at(31), &response(&[&HE_CAPABILITY, &HE_OPERATION]));
        assert!(world.station.is_connected());
        assert_eq!(world.station.peer().unwrap().associated.phy, PhyMode::Ht20);
        assert_eq!(connected_phy(&world), Some(PhyMode::Ht20));
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
            StaPhyRequest::PreferHe20 {
                ht40: true,
                power: power(),
            },
            &beacon,
        );
        assert_eq!(request_elements(&request), [(0, None), (1, None)]);
        world.receive(at(31), &open_association_response());
        assert!(world.station.is_connected());
        assert_eq!(connected_phy(&world), Some(PhyMode::Legacy));
    });
}
