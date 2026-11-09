//! The link lost and joined again: the access point deauthenticates the
//! station, the product's reconnect policy joins again under a new epoch,
//! and the network's DHCP client runs INIT-REBOOT (RFC 2131, 3.2 and
//! 4.4.2) to keep its lease.

use oer_portable_time::Duration;
use oer_protocols_ieee80211_runtime::station::contract::{DisconnectReason, Notice};
use oer_protocols_network_core::DhcpState;
use oer_protocols_network_port::LinkState;
use oer_protocols_network_wire::dhcp::MessageType;

use crate::simulation::access_point::ApEvent;
use crate::simulation::wired::{Answer, DhcpServer, LEASED_IP};
use crate::simulation::{Setup, World, simulate};

/// The station's reconnect interval.
const RECONNECT: Duration = Duration::from_secs(1);

fn reconnecting(dhcp: DhcpServer) -> Setup {
    Setup {
        dhcp,
        reconnect: Some(RECONNECT),
        ..Setup::default()
    }
}

/// The access point drops the station; the station hears it, its link goes
/// down, and the reconnect policy joins again: a second association, a
/// second handshake and the link up under a new epoch.
async fn lose_and_rejoin<T>(world: &World<'_, T>) {
    let epoch = world.link().epoch;
    world.ap().deauthenticate(2);
    world
        .until("the station hears it", Duration::from_secs(1), |world| {
            world.heard.disconnections() == 1
        })
        .await;
    assert!(world.heard.notices.borrow().iter().any(|(_, notice)| {
        *notice == Notice::Disconnected(DisconnectReason::Peer { reason_code: 2 })
    }));
    assert_eq!(world.link().state, LinkState::Down);
    world
        .until("the station joins again", Duration::from_secs(5), |world| {
            world.heard.connections() == 2 && world.ap().port_open()
        })
        .await;
    let link = world.link();
    assert_eq!(link.state, LinkState::Up);
    assert!(link.epoch > epoch, "a new link, a new epoch");
    assert_eq!(world.ap().associations(), 2);
}

/// After the rejoin the client asks to keep its lease (INIT-REBOOT: a
/// broadcast REQUEST naming its address and no server); the server's ACK
/// keeps the address, its version and the traffic on it.
#[test]
fn a_rejoin_keeps_the_lease_through_init_reboot() {
    struct Net;
    let report = simulate::<Net>(reconnecting(DhcpServer::default()), |world| {
        Box::pin(async move {
            let first = world.join().await;
            world.ap().wired.ping(1, 1, b"before");
            world
                .until("the first reply", Duration::from_secs(1), |world| {
                    world.ap().wired.echo_replies.len() == 1
                })
                .await;
            lose_and_rejoin(world).await;
            world
                .until("the lease is confirmed", Duration::from_secs(5), |world| {
                    world.ap().wired.dhcp.answered.len() == 3
                })
                .await;
            let reboot = world.ap().wired.dhcp.seen[2];
            assert_eq!(reboot.kind, MessageType::Request);
            assert_eq!(reboot.requested, Some(LEASED_IP));
            assert_eq!((reboot.ciaddr, reboot.server), ([0; 4], None));
            assert!(reboot.broadcast);
            world
                .until("bound again", Duration::from_secs(1), |world| {
                    world.configuration().and_then(|c| c.dhcp) == Some(DhcpState::Bound)
                })
                .await;
            let kept = world.configuration().expect("a configuration");
            assert_eq!(kept.ipv4, first.ipv4);
            assert_eq!(kept.version, first.version, "the address never changed");
            world.ap().wired.ping(1, 2, b"after");
            world
                .until(
                    "the reply on the new link",
                    Duration::from_secs(1),
                    |world| world.ap().wired.echo_replies.len() == 2,
                )
                .await;
            assert_eq!(world.ap().wired.echo_replies[1].data, b"after");
        })
    });
    report.assert_stopped_with_every_owner();
    let ap = report.access_point.borrow();
    assert_eq!(
        ap.wired.dhcp.kinds(),
        [
            MessageType::Discover,
            MessageType::Request,
            MessageType::Request
        ],
        "no discovery after the rejoin"
    );
    let authorized = ap
        .events
        .iter()
        .filter(|(_, event)| matches!(event, ApEvent::Authorized(_)))
        .count();
    assert_eq!(authorized, 2, "two handshakes");
}

/// The server NAKs the rejoined client's INIT-REBOOT, as for a network
/// that leases another address now: the address goes, discovery starts
/// over and the client leases the new address.
#[test]
fn a_nak_to_the_reboot_re_leases_a_new_address() {
    struct Net;
    const NEXT: [u8; 4] = [192, 0, 2, 101];
    let dhcp = DhcpServer {
        answer: Box::new(|seen| {
            if seen.requested == Some(LEASED_IP) && seen.server.is_none() {
                Answer::Nak
            } else {
                Answer::Ack
            }
        }),
        ..DhcpServer::default()
    };
    let report = simulate::<Net>(reconnecting(dhcp), |world| {
        Box::pin(async move {
            let first = world.join().await;
            world.ap().wired.dhcp.address = NEXT;
            lose_and_rejoin(world).await;
            world
                .until("the new lease", Duration::from_secs(5), |world| {
                    world.address() == Some(NEXT)
                })
                .await;
            let leased = world.configuration().expect("a configuration");
            assert_eq!(leased.dhcp, Some(DhcpState::Bound));
            assert_eq!(leased.version, first.version.wrapping_add(2));
            world.ap().wired.ping(3, 1, b"new address");
            world
                .until("the reply", Duration::from_secs(1), |world| {
                    world.ap().wired.echo_replies.len() == 1
                })
                .await;
            assert_eq!(world.ap().wired.echo_replies[0].source, NEXT);
        })
    });
    report.assert_stopped_with_every_owner();
    let ap = report.access_point.borrow();
    assert_eq!(
        ap.wired.dhcp.kinds(),
        [
            MessageType::Discover,
            MessageType::Request,
            MessageType::Request,
            MessageType::Discover,
            MessageType::Request,
        ]
    );
    assert_eq!(ap.wired.dhcp.answered[2].1, MessageType::Nak);
}
