//! What the core tells a caller's count, at the step that does the work:
//! each running place a readiness walk visits, each end an ended chain
//! tells, each frame built whole in the port's slot, and each lending
//! refusal for want of credit with the loans then out. Off, it tells nothing
//! and the same steps give the same answers.

use core::cell::Cell;
use std::vec::Vec;

use oer_protocols_network_port::Holding;

use super::*;
use crate::{Count, Intake};

/// A count a test switches.
#[derive(Default)]
struct Counter {
    on: Cell<bool>,
    asked: Cell<u32>,
    walked: Cell<u32>,
    told: Cell<u32>,
    materialised: Cell<u32>,
    no_credit: Cell<Vec<usize>>,
}

impl Counter {
    fn on() -> Self {
        let counter = Self::default();
        counter.on.set(true);
        counter
    }
}

fn bump(cell: &Cell<u32>) {
    cell.set(cell.get() + 1);
}

impl Count for Counter {
    fn on(&self) -> bool {
        bump(&self.asked);
        self.on.get()
    }

    fn walked(&self) {
        assert!(self.on.get(), "counted while off");
        bump(&self.walked);
    }

    fn told(&self) {
        assert!(self.on.get(), "counted while off");
        bump(&self.told);
    }

    fn materialised(&self) {
        assert!(self.on.get(), "counted while off");
        bump(&self.materialised);
    }

    fn no_credit(&self, loans: usize) {
        assert!(self.on.get(), "counted while off");
        let mut seen = self.no_credit.take();
        seen.push(loans);
        self.no_credit.set(seen);
    }
}

/// Four places a socket, the peer known.
type Wide<B> = NetworkOwner<'static, B, SIZE, 2, 4, 2, 2, BLOCKS, Port>;

fn wide<B: 'static>(test: impl FnOnce(&mut Wide<B>)) {
    let (pool, brand) = claimed::<B>();
    let mut stack = NetworkOwner::new(brand, interface(OUR_IP, 24, None), pool, Port::default());
    let request = arp_frame(Operation::Request, (PEER, PEER_IP), OUR_IP);
    stack.input(at(0), &request, &mut ());
    let reply = stack.poll_demand(at(0)).unwrap();
    stack
        .transmit(at(0), reply.grant(), unproduced, &())
        .unwrap();
    stack.port_mut().sent.clear();
    test(&mut stack);
}

fn send_to<B: core::fmt::Debug>(
    stack: &mut Wide<B>,
    socket: &crate::SocketId<B>,
    remote: Remote,
    data: &[u8],
) -> crate::CommandId<B> {
    let block = filled(stack.pool(), data);
    stack.send(at(1), socket, remote, block).unwrap()
}

/// Three datagrams for the known peer and one whose neighbor is not known.
fn four_running<B: core::fmt::Debug>(stack: &mut Wide<B>) -> crate::SocketId<B> {
    let socket = stack.open(5000, best_effort()).unwrap();
    let unknown = Remote {
        address: [192, 0, 2, 9],
        port: 7,
    };
    let _waits = send_to(stack, &socket, unknown, b"waits");
    for data in [b"a", b"b", b"c"] {
        let _ready = send_to(stack, &socket, peer(), data);
    }
    socket
}

#[test]
fn a_readiness_walk_counts_every_running_place_it_visits() {
    brand!(Net);
    wide::<Net>(|stack| {
        let _socket = four_running(stack);
        let counter = Counter::on();
        let ready: Vec<_> = stack
            .demands(at(1), &counter)
            .map(|demand| demand.ready().class)
            .collect();
        assert_eq!(
            ready,
            [Class::Control, Class::Data, Class::Data, Class::Data],
            "the ARP request, then the three ready"
        );
        assert_eq!(counter.walked.get(), 4, "the one waiting is visited too");
        // A second pass over a clone of the walk visits them again.
        let walk = stack.demands(at(1), &counter);
        assert_eq!(walk.clone().count() + walk.count(), 8);
        assert_eq!(counter.walked.get(), 12);
        // A walk that stops early visits only what it reached.
        let _first_data = stack
            .demands(at(1), &counter)
            .find(|demand| demand.ready().class == Class::Data);
        assert_eq!(
            counter.walked.get(),
            14,
            "the waiting one, then the first ready"
        );
    });
}

#[test]
fn ends_and_frames_count_where_they_are_told_and_built() {
    brand!(Net);
    wide::<Net>(|stack| {
        let socket = four_running(stack);
        let ends = socket.ends();
        let counter = Counter::on();
        // The ARP request and the three datagrams, each built whole.
        for _ in 0..4 {
            let demand = stack.poll_demand(at(1)).unwrap();
            stack
                .transmit(at(1), demand.grant(), unproduced, &counter)
                .unwrap();
        }
        assert_eq!(counter.materialised.get(), 4);
        // A stale grant builds nothing.
        let stale = stack.poll_demand(at(1)).map(|demand| demand.grant());
        assert!(stale.is_none(), "the waiting one is not ready");
        let told: Vec<_> = core::iter::from_fn(|| stack.next_end(&ends, &counter)).collect();
        assert_eq!(told.len(), 3);
        assert_eq!(
            counter.told.get(),
            3,
            "one a told end, none for the empty look"
        );
        // A short slot builds no frame either.
        let socket = stack.open(5001, best_effort()).unwrap();
        let _short = send_to(stack, &socket, peer(), b"too long for the room");
        stack.port_mut().room = Some(20);
        let demand = stack.poll_demand(at(1)).unwrap();
        assert_eq!(
            stack.transmit(at(1), demand.grant(), unproduced, &counter),
            Err(Unsent::SlotTooShort)
        );
        assert_eq!(counter.materialised.get(), 4);
    });
}

/// Four works through the core: each step's answers, then the port's frames.
fn script<B: core::fmt::Debug>(counter: &Counter, stack: &mut Wide<B>) -> Vec<std::string::String> {
    let socket = four_running(stack);
    let ends = socket.ends();
    let mut seen = Vec::new();
    for _ in 0..4 {
        let demands: Vec<_> = stack
            .demands(at(1), counter)
            .map(|demand| *demand.ready())
            .collect();
        seen.push(std::format!("{demands:?}"));
        let demand = stack.demands(at(1), counter).next().unwrap();
        let sent = stack.transmit(at(1), demand.grant(), unproduced, counter);
        seen.push(std::format!("{sent:?}"));
    }
    let told: Vec<_> = core::iter::from_fn(|| stack.next_end(&ends, counter)).collect();
    seen.push(std::format!("{told:?}"));
    seen.push(std::format!("{:?}", stack.port_mut().sent));
    seen
}

#[test]
fn a_count_that_is_off_hears_nothing_and_changes_nothing() {
    brand!(Off);
    brand!(On);
    let off = Counter::default();
    let mut off_seen = Vec::new();
    wide::<Off>(|stack| off_seen = script(&off, stack));
    assert!(off.asked.get() > 0, "an enabled count is asked");
    let on = Counter::on();
    let mut on_seen = Vec::new();
    wide::<On>(|stack| on_seen = script(&on, stack));
    assert_eq!(on_seen, off_seen, "the same work, frames and ends");
    assert!(on.walked.get() > 0 && on.told.get() == 3 && on.materialised.get() == 4);
}

#[test]
fn a_refusal_for_credit_counts_the_loans_then_out() {
    #[derive(Debug, Eq, PartialEq)]
    struct Net;
    run::<Net>(interface(OUR_IP, 24, None), |stack| {
        stack.pool().loans().pool(4, 1).unwrap();
        let group = stack.pool().group(4).unwrap();
        let _socket = stack.open_lending(9, best_effort(), group, 1).unwrap();
        for data in [b"first", b"secnd"] {
            stack
                .port_mut()
                .received
                .push_back((udp_frame(7, 9, data), Holding::Lendable));
        }
        let counter = Counter::on();
        let mut dropped = Dropped::default();
        let Intake::Lent(first) = stack.take_frame(at(0), &mut dropped, &counter) else {
            panic!("the credit takes one");
        };
        assert!(counter.no_credit.take().is_empty(), "a loan is no refusal");
        assert!(matches!(
            stack.take_frame(at(0), &mut dropped, &counter),
            Intake::Read
        ));
        assert_eq!(counter.no_credit.take(), [1], "one refusal, one loan out");
        stack.pool().loans().give_back(first.loan);
    });
}
