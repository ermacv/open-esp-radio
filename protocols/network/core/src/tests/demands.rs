//! The walk of ready work against the chain of iterators it replaced: the
//! same pieces of work, in the same order, over every source at once and
//! after every publication moves the sockets' turn.

use super::*;

/// The walk and the reference over the same state.
fn same_demands<B: core::fmt::Debug>(stack: &Stack<B>, now: Instant) -> usize {
    let walked: Vec<_> = stack.demands(now, &()).collect();
    let chained: Vec<_> = stack.demands_chain(now, &()).collect();
    assert_eq!(std::format!("{walked:?}"), std::format!("{chained:?}"));
    walked.len()
}

#[test]
fn the_walk_of_ready_work_lists_what_the_chain_listed() {
    world!(|stack| {
        knows_peer(stack);
        let first = stack.open(5000, best_effort()).unwrap();
        let second = stack.open(5001, best_effort()).unwrap();
        let unknown = Remote {
            address: [192, 0, 2, 9],
            port: 7,
        };
        for (socket, remote, data) in [
            (&first, peer(), &b"one"[..]),
            (&first, peer(), &b"uno"[..]),
            (&second, peer(), &b"two"[..]),
            (&second, unknown, &b"waits"[..]),
        ] {
            let block = filled(stack.pool(), data);
            let _command = stack.send(at(1), socket, remote, block).unwrap();
        }
        // An ARP reply owed to another asker, and an echo owed to the peer.
        let asker = arp_frame(
            Operation::Request,
            ([2, 0, 0, 0, 0, 3], [192, 0, 2, 3]),
            OUR_IP,
        );
        stack.input(at(1), &asker, &mut ());
        let ping = super::echo::icmp_frame(OUR_IP, &super::echo::echo(1, b"ping"));
        stack.input(at(1), &ping, &mut ());
        let mut listed = same_demands(stack, at(1));
        assert!(listed >= 5, "every source has work: {listed}");
        // Each publication moves the sockets' turn and takes one piece away.
        while let Some(demand) = stack.poll_demand(at(1)) {
            let _ = stack.transmit(at(1), demand.grant(), unproduced, &());
            listed = same_demands(stack, at(1));
        }
        assert_eq!(
            listed, 0,
            "a datagram waiting for its neighbor is not ready"
        );
    });
}
