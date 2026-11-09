//! The join, from the scan to the lease, and the echo service on the
//! leased address.

use oer_portable_time::Duration;
use oer_protocols_network_core::DhcpState;
use oer_protocols_network_wire::dhcp::MessageType;

use crate::simulation::access_point::{ApEvent, BSSID};
use crate::simulation::wired::{LEASED_IP, PEER_IP, ROUTER_IP};
use crate::simulation::{Setup, simulate};
use oer_protocols_ieee80211_runtime::station::OperationEnd;
use oer_protocols_ieee80211_runtime::station::contract::{CommandOutcome, DisconnectReason};

/// The station scans, authenticates, associates and completes the four-way
/// handshake, which the access point's authenticator verifies; the port
/// opens, and the network's DHCP client leases the server's address in one
/// DISCOVER, OFFER, REQUEST and ACK. The graceful stop then leaves the
/// network and brings every owner home.
#[test]
fn a_station_joins_through_the_handshake_and_leases_its_address() {
    struct Net;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            let configuration = world.join().await;
            let ipv4 = configuration.ipv4.expect("an address");
            assert_eq!(ipv4.address(), LEASED_IP);
            assert_eq!(ipv4.prefix(), 24);
            assert_eq!(ipv4.gateway(), Some(ROUTER_IP));
            assert_eq!(configuration.dhcp, Some(DhcpState::Bound));
            let lease = configuration.lease.expect("a lease");
            assert_eq!(lease.server, ROUTER_IP);
            assert_eq!(
                lease.expires,
                lease.obtained.saturating_add(Duration::from_secs(3600))
            );
            assert_eq!(
                world.link().state,
                oer_protocols_network_port::LinkState::Up
            );
        })
    });
    report.assert_stopped_with_every_owner();
    let ap = report.access_point.borrow();
    let events: Vec<_> = ap.events.iter().map(|(_, event)| event.clone()).collect();
    assert_eq!(
        events,
        [
            ApEvent::Probed,
            ApEvent::Authenticated,
            ApEvent::Associated(1),
            ApEvent::Authorized(1),
            // The graceful stop's Deauthentication: leaving.
            ApEvent::Left(3),
        ]
    );
    let dhcp = &ap.wired.dhcp;
    assert_eq!(dhcp.kinds(), [MessageType::Discover, MessageType::Request]);
    let request = dhcp.seen[1];
    assert_eq!(request.xid, dhcp.seen[0].xid, "one transaction");
    assert_eq!(request.requested, Some(LEASED_IP));
    assert_eq!(request.server, Some(ROUTER_IP));
    assert!(request.broadcast, "SELECTING broadcasts its REQUEST");
    assert_eq!(
        ap.dropped_from_station, 0,
        "nothing went before the port opened"
    );
    let notices = report.heard.notices.borrow();
    assert!(matches!(
        notices.first(),
        Some((
            _,
            oer_protocols_ieee80211_runtime::station::contract::Notice::Connected {
                bssid: BSSID,
                channel: 6,
                ..
            }
        ))
    ));
}

/// The peer pings the leased address: the network core answers each echo
/// request with its identifier, sequence and data, after resolving the
/// peer by ARP once.
#[test]
fn the_station_answers_echo_requests_on_its_leased_address() {
    struct Net;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            world.join().await;
            for sequence in 0..4_u16 {
                let data = [sequence as u8; 56];
                world.ap().wired.ping(0x0e0e, sequence, &data);
                world
                    .until("the echo reply", Duration::from_secs(1), |world| {
                        world.ap().wired.echo_replies.len() == usize::from(sequence) + 1
                    })
                    .await;
                let ap = world.ap();
                let reply = ap.wired.echo_replies.last().expect("a reply");
                assert_eq!(reply.source, LEASED_IP);
                assert_eq!((reply.identifier, reply.sequence), (0x0e0e, sequence));
                assert_eq!(reply.data, data);
            }
            assert_eq!(world.ap().wired.arp_answered, [PEER_IP], "one resolution");
        })
    });
    report.assert_stopped_with_every_owner();
}

/// The station's profile names another PMK: the access point's
/// authenticator drops Message 2, whose MIC fails, the station's handshake
/// times out and its connect ends failed. The port never opens, nothing reaches the
/// wired side and no address is leased; the graceful stop of the station,
/// now without a link, still ends and brings every owner home.
#[test]
fn a_wrong_key_never_opens_the_port_and_the_connect_fails() {
    struct Net;
    let setup = Setup {
        pmk: [9; 32],
        ..Setup::default()
    };
    let report = simulate::<Net>(setup, |world| {
        Box::pin(async move {
            let end = world.connect().await;
            assert!(
                matches!(
                    end,
                    OperationEnd::Completed(CommandOutcome::ConnectFailed(
                        DisconnectReason::HandshakeFailed
                    ))
                ),
                "{end:?}"
            );
            assert!(!world.ap().port_open());
            assert_eq!(world.address(), None);
        })
    });
    report.assert_stopped_with_every_owner();
    let ap = report.access_point.borrow();
    let events: Vec<_> = ap.events.iter().map(|(_, event)| event.clone()).collect();
    // The authenticator drops the Message 2 whose MIC fails; the station's
    // handshake times out and it leaves, 4-way handshake timeout.
    assert_eq!(
        events,
        [
            ApEvent::Probed,
            ApEvent::Authenticated,
            ApEvent::Associated(1),
            ApEvent::Left(15),
        ]
    );
    assert!(ap.wired.dhcp.seen.is_empty(), "no DHCP message got through");
}

/// The graceful stop of a station that never had a link ends: the stop's
/// Disconnect is refused (`NotConnected`), and the lifecycle takes the
/// policy's Stop though the refusal's wake of the policy came before the
/// lifecycle registered for it.
#[test]
fn a_graceful_stop_of_a_station_without_a_link_ends() {
    struct Net;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            world.stop(Duration::from_secs(10));
        })
    });
    report.assert_stopped_with_every_owner();
}

/// The association's slowest data rate reaches the network side with the
/// link: the access point's set (1, 2, 5.5 and 11 Mb/s) ends at DSSS 1 Mb/s,
/// where the station's least grants start.
#[test]
fn the_link_carries_the_association_s_slowest_rate() {
    use oer_protocols_ieee80211_wire::phy::{LegacyRate, PhyRate};
    struct Net;
    let report = simulate::<Net>(Setup::default(), |world| {
        Box::pin(async move {
            assert_eq!(world.link_floor(), None, "nothing before an association");
            world.join().await;
            assert_eq!(
                world.link_floor(),
                Some(PhyRate::Legacy(LegacyRate::Dsss1M))
            );
        })
    });
    report.assert_stopped_with_every_owner();
}
