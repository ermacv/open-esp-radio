//! Each work's one record: its end told once, by place and generation, the
//! told end no right to its result, and running work offered in the order it
//! was accepted.

use std::vec::Vec;

use super::*;
use crate::{CommandId, Ends, WorkId};

/// Four places a socket, to see an order among them.
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

fn told<B, P: FramePort, const S: usize, const C: usize>(
    stack: &mut NetworkOwner<'static, B, SIZE, S, C, 2, 2, BLOCKS, P>,
    ends: &Ends<B>,
) -> Vec<WorkId> {
    core::iter::from_fn(|| stack.next_end(ends, &())).collect()
}

fn send_wide<B: core::fmt::Debug>(
    stack: &mut Wide<B>,
    socket: &crate::SocketId<B>,
    data: &[u8],
) -> CommandId<B> {
    let block = filled(stack.pool(), data);
    stack.send(at(1), socket, peer(), block).unwrap()
}

fn transmit_next<B>(stack: &mut Wide<B>) {
    let demand = stack.poll_demand(at(1)).unwrap();
    stack
        .transmit(at(1), demand.grant(), unproduced, &())
        .unwrap();
}

#[test]
fn each_end_is_told_once_in_the_order_it_came() {
    brand!(Net);
    wide::<Net>(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let ends = socket.ends();
        let first = send_wide(stack, &socket, b"one");
        let second = send_wide(stack, &socket, b"two");
        let third = send_wide(stack, &socket, b"three");
        assert!(told(stack, &ends).is_empty(), "running work is not told");
        transmit_next(stack);
        assert_eq!(told(stack, &ends), [first.work()]);
        transmit_next(stack);
        transmit_next(stack);
        assert_eq!(told(stack, &ends), [second.work(), third.work()]);
        assert!(told(stack, &ends).is_empty(), "told once");
        // A told end is no right: the result waits for its command.
        for command in [first, second, third] {
            assert_eq!(finished(stack.take_result(command)), Outcome::Accepted);
        }
        assert!(told(stack, &ends).is_empty());
    });
}

#[test]
fn an_end_met_again_is_not_told_again() {
    brand!(Net);
    wide::<Net>(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let ends = socket.ends();
        let command = send_wide(stack, &socket, b"evicted");
        let stale = stack.poll_demand(at(1)).unwrap().grant();
        let queued = stack.queued().next().unwrap();
        assert!(stack.evict(queued, Eviction::MemoryPressure));
        // Its grant and its socket's close meet an end already.
        assert_eq!(
            stack.transmit(at(1), stale, unproduced, &()),
            Err(Unsent::Stale)
        );
        assert!(stack.queued().next().is_none());
        let close = stack.close(socket).unwrap();
        assert_eq!(told(stack, &ends), [command.work(), close.work()]);
        assert_eq!(
            finished(stack.take_result(command)),
            Outcome::Evicted(Eviction::MemoryPressure)
        );
        assert_eq!(finished(stack.take_result(close)), Outcome::SocketClosed);
    });
}

#[test]
fn a_result_read_before_its_end_is_told_is_never_told() {
    brand!(Net);
    wide::<Net>(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let ends = socket.ends();
        let read = send_wide(stack, &socket, b"read");
        transmit_next(stack);
        let place = read.work().place();
        assert_eq!(finished(stack.take_result(read)), Outcome::Accepted);
        let unobserved = send_wide(stack, &socket, b"unobserved");
        transmit_next(stack);
        stack.drop_observer(unobserved);
        assert!(told(stack, &ends).is_empty());
        // The place serves again, under another generation.
        let next = send_wide(stack, &socket, b"next");
        assert_eq!(next.work().place(), place, "the first place freed");
        transmit_next(stack);
        assert_eq!(told(stack, &ends), [next.work()]);
        assert_eq!(finished(stack.take_result(next)), Outcome::Accepted);
    });
}

#[test]
fn a_told_end_names_its_generation_and_an_old_right_finds_nothing() {
    brand!(Net);
    wide::<Net>(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let ends = socket.ends();
        let old = send_wide(stack, &socket, b"old");
        transmit_next(stack);
        assert_eq!(told(stack, &ends), [old.work()]);
        let old_work = old.work();
        assert_eq!(finished(stack.take_result(old)), Outcome::Accepted);
        let new = send_wide(stack, &socket, b"new");
        transmit_next(stack);
        let [end] = told(stack, &ends)[..] else {
            panic!("one end");
        };
        assert_eq!(end.place(), old_work.place());
        assert_ne!(end, old_work, "another generation");
        assert_eq!(end, new.work());
    });
}

#[test]
fn the_ends_of_a_closed_socket_tell_nothing_of_the_next_in_its_place() {
    small!(|stack| {
        let first = stack.open(5000, best_effort()).unwrap();
        let old = first.ends();
        let close = stack.close(first).unwrap();
        assert_eq!(core::iter::from_fn(|| stack.next_end(&old, &())).count(), 1);
        assert_eq!(finished(stack.take_result(close)), Outcome::SocketClosed);
        let second = stack.open(5001, best_effort()).unwrap();
        let block = stack.pool().take().unwrap();
        let broadcast = Remote {
            address: [192, 0, 2, 255],
            port: 7,
        };
        let command = stack.send(at(0), &second, broadcast, block).unwrap();
        let demand = stack.poll_demand(at(0)).unwrap();
        stack
            .transmit(at(0), demand.grant(), unproduced, &())
            .unwrap();
        assert_eq!(stack.next_end(&old, &()), None, "another socket's end");
        assert_eq!(stack.next_end(&second.ends(), &()), Some(command.work()));
    });
}

#[test]
fn running_work_is_offered_in_the_order_it_was_accepted() {
    brand!(Net);
    wide::<Net>(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let first = send_wide(stack, &socket, b"first");
        let second = send_wide(stack, &socket, b"second");
        transmit_next(stack);
        assert_eq!(finished(stack.take_result(first)), Outcome::Accepted);
        // The third takes the first's place, and goes after the second.
        let third = send_wide(stack, &socket, b"third");
        assert_eq!(third.work().place(), 0);
        let order: Vec<_> = stack
            .demands(at(1), &())
            .map(|demand| demand.ready().length)
            .collect();
        assert_eq!(order, [14 + 20 + 8 + 6, 14 + 20 + 8 + 5]);
        transmit_next(stack);
        transmit_next(stack);
        assert_eq!(finished(stack.take_result(second)), Outcome::Accepted);
        assert_eq!(finished(stack.take_result(third)), Outcome::Accepted);
        let sent = &stack.port_mut().sent;
        assert!(sent[1].ends_with(b"second") && sent[2].ends_with(b"third"));
    });
}

#[test]
fn a_close_tells_every_running_end_before_its_own() {
    brand!(Net);
    wide::<Net>(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let ends = socket.ends();
        let commands: Vec<_> = (0..4_u8).map(|n| send_wide(stack, &socket, &[n])).collect();
        let close = stack.close(socket).unwrap();
        let mut expected: Vec<_> = commands.iter().map(CommandId::work).collect();
        expected.push(close.work());
        assert_eq!(told(stack, &ends), expected);
        for command in commands {
            assert_eq!(finished(stack.take_result(command)), Outcome::Closed);
        }
        assert_eq!(stack.pool().free(), BLOCKS);
    });
}

#[test]
fn acceptance_order_is_no_send_order_a_ready_datagram_goes_before_one_waiting_for_arp() {
    brand!(Net);
    wide::<Net>(|stack| {
        let socket = stack.open(5000, best_effort()).unwrap();
        let unknown = Remote {
            address: [192, 0, 2, 9],
            port: 7,
        };
        let block = filled(stack.pool(), b"waits");
        let waiting = stack.send(at(1), &socket, unknown, block).unwrap();
        let ready = send_wide(stack, &socket, b"ready");
        // The ARP request for the unknown neighbor, then the ready datagram:
        // the earlier one is not ready, so it is not offered.
        let order: Vec<_> = stack
            .demands(at(1), &())
            .map(|demand| demand.ready().class)
            .collect();
        assert_eq!(order, [Class::Control, Class::Data]);
        transmit_next(stack);
        transmit_next(stack);
        assert_eq!(finished(stack.take_result(ready)), Outcome::Accepted);
        assert!(matches!(
            stack.take_result(waiting),
            CommandState::Running(_)
        ));
    });
}
