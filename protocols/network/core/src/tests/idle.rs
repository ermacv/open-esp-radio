//! A core that is idle has no work ready, certainly: each source the list
//! of ready work takes from keeps it from being idle on its own, and once
//! its work went the core is idle again.

use super::echo::{echo, icmp_frame};
use super::*;

/// The kinds of work the list holds now.
fn listed<B>(stack: &Stack<B>, now: Instant) -> Vec<Class> {
    stack
        .demands(now, &())
        .map(|demand| demand.ready().class)
        .collect()
}

#[test]
fn a_new_core_is_idle() {
    world!(|stack| {
        assert!(stack.idle());
        assert!(listed(stack, at(0)).is_empty());
    });
}

#[test]
fn an_owed_arp_reply_alone_keeps_the_core_from_idle() {
    world!(|stack| {
        let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
        stack.input(at(0), &request, &mut ());
        assert_eq!(listed(stack, at(0)), [Class::Control]);
        assert!(!stack.idle(), "a reply is owed");
        send_next(stack, at(0)).unwrap();
        assert!(stack.idle());
    });
}

#[test]
fn an_owed_echo_reply_alone_keeps_the_core_from_idle() {
    world!(|stack| {
        knows_peer(stack);
        assert!(stack.idle());
        stack.input(at(0), &icmp_frame(OUR_IP, &echo(1, b"ping")), &mut ());
        assert_eq!(listed(stack, at(0)), [Class::Control]);
        assert!(!stack.idle(), "an echo reply is owed");
        send_next(stack, at(0)).unwrap();
        assert!(stack.idle());
    });
}

#[test]
fn an_owed_dhcp_message_alone_keeps_the_core_from_idle() {
    world_on!(Interface::dhcp(LOCAL, 1500, 0x5EED).unwrap(), |stack| {
        assert!(stack.idle(), "no link, no client yet");
        stack.port_mut().up = Some(1);
        stack.observe_link(at(0));
        stack.tick(at(0), &mut ());
        assert_eq!(listed(stack, at(0)), [Class::Control]);
        assert!(!stack.idle(), "a discover is owed");
        send_next(stack, at(0)).unwrap();
        assert!(stack.idle(), "until its time comes again");
    });
}

#[test]
fn a_running_datagram_alone_keeps_the_core_from_idle() {
    world!(|stack| {
        knows_peer(stack);
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"hello");
        let _command = stack.send(at(0), &socket, peer(), block).unwrap();
        assert_eq!(listed(stack, at(0)), [Class::Data]);
        assert!(!stack.idle(), "a datagram runs");
        send_next(stack, at(0)).unwrap();
        assert!(stack.idle());
    });
}

#[test]
fn a_declared_work_alone_keeps_the_core_from_idle() {
    world!(|stack| {
        let socket = stack.open_local(6000, best_effort()).unwrap();
        let broadcast = Remote {
            address: [192, 0, 2, 255],
            port: 7,
        };
        let _work = stack.declare(at(0), &socket, broadcast, 10, 1).unwrap();
        assert_eq!(listed(stack, at(0)), [Class::Data]);
        assert!(!stack.idle(), "a producer's work runs");
        let demand = stack.poll_demand(at(0)).unwrap();
        stack
            .transmit(
                at(0),
                demand.grant(),
                |_, _, data: &mut crate::Payload<'_>| {
                    data.repeat(0, 10).map_err(|_| crate::Declined::Unwritable)
                },
                &(),
            )
            .unwrap();
        assert!(stack.idle());
    });
}

/// A running datagram whose neighbor is not known is not ready, and the core
/// is not idle for it either: idle is certain, never a guess.
#[test]
fn work_that_is_not_ready_yet_keeps_the_core_from_idle() {
    world!(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let block = filled(stack.pool(), b"hello");
        let _command = stack.send(at(0), &socket, peer(), block).unwrap();
        send_next(stack, at(0)).unwrap();
        assert!(listed(stack, at(0)).is_empty(), "it waits for its neighbor");
        assert!(!stack.idle());
    });
}
