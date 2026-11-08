//! The DHCP lease's life (RFC 2131, 4.4.5): renewal at T1, rebinding at T2,
//! the end of an unanswered lease and a NAK.

use std::cell::Cell;
use std::rc::Rc;

use oer_portable_time::Duration;
use oer_protocols_network_core::DhcpState;
use oer_protocols_network_wire::dhcp::MessageType;

use crate::simulation::wired::{Answer, DhcpServer, LEASED_IP};
use crate::simulation::{Setup, simulate};

/// A lease short enough for its times to come within a case.
const LEASE_SECS: u32 = 600;

fn short_lease(
    answer: impl FnMut(&crate::simulation::wired::DhcpSeen) -> Answer + 'static,
) -> Setup {
    Setup {
        dhcp: DhcpServer {
            lease_secs: LEASE_SECS,
            answer: Box::new(answer),
            ..DhcpServer::default()
        },
        ..Setup::default()
    }
}

/// At T1 the client renews by unicast to its server, from its address and
/// naming no server; the ACK extends the lease from that REQUEST and the
/// address, its version and the traffic on it stay.
#[test]
fn the_client_renews_at_t1_by_unicast_and_keeps_its_address() {
    struct Net;
    let report = simulate::<Net>(short_lease(|_| Answer::Ack), |world| {
        Box::pin(async move {
            let first = world.join().await;
            let lease = first.lease.expect("a lease");
            assert_eq!(
                lease.renew,
                lease.obtained.saturating_add(Duration::from_secs(300))
            );
            world
                .until("the renewal", Duration::from_secs(400), |world| {
                    world
                        .configuration()
                        .and_then(|configuration| configuration.lease)
                        .is_some_and(|renewed| renewed.obtained != lease.obtained)
                })
                .await;
            let renewed = world.configuration().expect("a configuration");
            assert_eq!(renewed.dhcp, Some(DhcpState::Bound));
            assert_eq!(renewed.ipv4, first.ipv4);
            assert_eq!(renewed.version, first.version, "the address did not change");
            let renewal = world.ap().wired.dhcp.seen[2];
            assert_eq!(renewal.kind, MessageType::Request);
            assert_eq!(renewal.ciaddr, LEASED_IP);
            assert_eq!((renewal.requested, renewal.server), (None, None));
            assert!(!renewal.broadcast, "RENEWING unicasts to the server");
            assert!(renewal.at >= lease.renew && renewal.at < lease.rebind);
            let obtained = renewed.lease.expect("the lease").obtained;
            assert_eq!(obtained, renewal.at, "the lease counts from its REQUEST");
            // The address still carries traffic.
            world.ap().wired.ping(1, 1, b"renewed");
            world
                .until("the echo reply", Duration::from_secs(1), |world| {
                    world.ap().wired.echo_replies.len() == 1
                })
                .await;
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
        ]
    );
}

/// The server never answers a renewal: at T2 the client rebinds by
/// broadcast, and that ACK keeps the address.
#[test]
fn an_unanswered_renewal_rebinds_at_t2_by_broadcast() {
    struct Net;
    let setup = short_lease(|seen| {
        if seen.broadcast {
            Answer::Ack
        } else {
            Answer::Silent
        }
    });
    let report = simulate::<Net>(setup, |world| {
        Box::pin(async move {
            let first = world.join().await;
            let lease = first.lease.expect("a lease");
            world
                .until("the rebinding", Duration::from_secs(LEASE_SECS), |world| {
                    world
                        .configuration()
                        .and_then(|configuration| configuration.lease)
                        .is_some_and(|renewed| renewed.obtained != lease.obtained)
                })
                .await;
            let rebound = world.configuration().expect("a configuration");
            assert_eq!(rebound.ipv4, first.ipv4);
            assert_eq!(rebound.version, first.version);
            assert_eq!(rebound.dhcp, Some(DhcpState::Bound));
            let seen = world.ap().wired.dhcp.seen.clone();
            let renewals: Vec<_> = seen[2..].iter().filter(|seen| !seen.broadcast).collect();
            assert!(!renewals.is_empty(), "the client renewed first");
            assert!(renewals.iter().all(|seen| seen.at < lease.rebind));
            let rebinding = seen.last().expect("the rebinding");
            assert!(rebinding.broadcast, "REBINDING broadcasts");
            assert_eq!(rebinding.kind, MessageType::Request);
            assert_eq!(rebinding.ciaddr, LEASED_IP);
            assert!(rebinding.at >= lease.rebind && rebinding.at < lease.expires);
        })
    });
    report.assert_stopped_with_every_owner();
}

/// The server answers nothing after its first ACK: the address goes at the
/// lease's end, in that instant, and the client starts discovery over.
#[test]
fn a_lease_no_server_extends_ends_and_discovery_starts_over() {
    struct Net;
    let acked = Rc::new(Cell::new(false));
    let setup = {
        let acked = acked.clone();
        short_lease(move |_| {
            if acked.replace(true) {
                Answer::Silent
            } else {
                Answer::Ack
            }
        })
    };
    let report = simulate::<Net>(setup, |world| {
        Box::pin(async move {
            let first = world.join().await;
            let lease = first.lease.expect("a lease");
            world.ap().wired.dhcp.offers = false;
            world
                .until(
                    "the lease's end",
                    Duration::from_secs(LEASE_SECS + 1),
                    |world| world.address().is_none(),
                )
                .await;
            assert_eq!(world.now(), lease.expires, "the address goes at the end");
            let ended = world.configuration().expect("a configuration");
            assert_ne!(ended.version, first.version);
            assert_eq!(ended.lease, None);
            world
                .until("discovery", Duration::from_secs(1), |world| {
                    world.ap().wired.dhcp.kinds().last() == Some(&MessageType::Discover)
                })
                .await;
            assert_eq!(
                world.configuration().and_then(|c| c.dhcp),
                Some(DhcpState::Selecting)
            );
        })
    });
    report.assert_stopped_with_every_owner();
}

/// The server NAKs the renewal, as for an address it no longer grants: the
/// address goes, discovery starts over at once, and the client leases the
/// address the server offers now.
#[test]
fn a_nak_to_the_renewal_ends_the_address_and_the_client_leases_anew() {
    struct Net;
    const NEXT: [u8; 4] = [192, 0, 2, 101];
    let report = simulate::<Net>(
        short_lease(|seen| {
            if seen.ciaddr == LEASED_IP {
                Answer::Nak
            } else {
                Answer::Ack
            }
        }),
        |world| {
            Box::pin(async move {
                let first = world.join().await;
                let lease = first.lease.expect("a lease");
                world.ap().wired.dhcp.address = NEXT;
                world
                    .until("the new lease", Duration::from_secs(400), |world| {
                        world.address() == Some(NEXT)
                    })
                    .await;
                let answered = world.ap().wired.dhcp.answered.clone();
                let (nak_at, _) = answered
                    .iter()
                    .copied()
                    .find(|(_, kind)| *kind == MessageType::Nak)
                    .expect("the NAK");
                assert!(nak_at >= lease.renew, "the renewal drew it");
                // The NAK ended the old address before the new lease: two
                // changes of address.
                let renewed = world.configuration().expect("a configuration");
                assert_eq!(renewed.dhcp, Some(DhcpState::Bound));
                assert_eq!(renewed.version, first.version.wrapping_add(2));
                assert!(renewed.lease.expect("a lease").obtained >= nak_at);
                // The new address carries traffic.
                world.ap().wired.ping(2, 1, b"anew");
                world
                    .until("the echo reply", Duration::from_secs(1), |world| {
                        world.ap().wired.echo_replies.len() == 1
                    })
                    .await;
                assert_eq!(world.ap().wired.echo_replies[0].source, NEXT);
            })
        },
    );
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
}
