//! A lending socket: what its datagrams keep, and what each refusal says.

use oer_protocols_network_port::{CopyReason, EthernetFrame, Holding};

use super::{Dropped, OUR_IP, PEER_IP, Stack, at, best_effort, interface, run, udp_frame};
use crate::tests::Took;
use crate::{LoansRefused, NetEvent, NotLent, OpenRefused, Remote};

const PORT: u16 = 9;

fn arrives<B>(stack: &mut Stack<B>, data: &[u8], holding: Holding) {
    stack
        .port_mut()
        .received
        .push_back((udp_frame(7, PORT, data), holding));
}

macro_rules! lending {
    ($test:expr) => {{
        #[derive(Debug, Eq, PartialEq)]
        struct Net;
        run::<Net>(interface(OUR_IP, 24, None), $test);
    }};
}

#[test]
fn a_lendable_datagram_stays_in_the_port_s_buffer_under_a_loan() {
    lending!(|stack| {
        stack.pool().loans().pool(4, 1).unwrap();
        let group = stack.pool().group(4).unwrap();
        let socket = stack.open_lending(PORT, best_effort(), group, 2).unwrap();
        let free = stack.pool().free();
        arrives(stack, b"lent", Holding::Lendable);
        let mut dropped = Dropped::default();
        let Took::Lent(lent) = stack.take_one(at(0), &mut dropped, &()) else {
            panic!("a lendable datagram is lent");
        };
        assert_eq!(lent.flow, socket.flow());
        assert_eq!(
            lent.remote,
            Remote {
                address: PEER_IP,
                port: 7
            }
        );
        let mut frame = lent.frame;
        let view = frame.view().unwrap();
        assert_eq!(view.payload.get(lent.data.clone()), Some(&b"lent"[..]));
        assert_eq!(stack.pool().free(), free, "no block holds it");
        assert_eq!(stack.pool().held(group), 0);
        assert_eq!(stack.pool().loans().outstanding(), 1);
        assert!(dropped.0.is_empty());
        stack.pool().loans().give_back(lent.loan);
        assert!(stack.pool().loans().released());
    });
}

#[test]
fn a_socket_at_its_credit_drops_with_no_credit_and_the_port_keeps_the_buffer() {
    lending!(|stack| {
        stack.pool().loans().pool(4, 1).unwrap();
        let group = stack.pool().group(4).unwrap();
        let socket = stack.open_lending(PORT, best_effort(), group, 1).unwrap();
        arrives(stack, b"first", Holding::Lendable);
        arrives(stack, b"second", Holding::Lendable);
        let mut dropped = Dropped::default();
        let Took::Lent(first) = stack.take_one(at(0), &mut dropped, &()) else {
            panic!("the credit takes one");
        };
        assert!(matches!(
            stack.take_one(at(0), &mut dropped, &()),
            Took::Read
        ));
        assert_eq!(dropped.0, [NetEvent::NotLent(NotLent::NoCredit)]);
        assert_eq!(stack.pool().loans().outstanding(), 1);
        assert!(
            matches!(stack.receive(&socket), Ok(None)),
            "nothing was copied"
        );
        // The returned loan frees the credit for the next.
        stack.pool().loans().give_back(first.loan);
        arrives(stack, b"third", Holding::Lendable);
        assert!(matches!(
            stack.take_one(at(0), &mut dropped, &()),
            Took::Lent(_)
        ));
    });
}

#[test]
fn a_pressure_copy_is_dropped_and_a_format_copy_goes_into_the_socket_s_group() {
    lending!(|stack| {
        stack.pool().loans().pool(4, 1).unwrap();
        let group = stack.pool().group(4).unwrap();
        let socket = stack.open_lending(PORT, best_effort(), group, 1).unwrap();
        let free = stack.pool().free();
        arrives(stack, b"pressed", Holding::Copied(CopyReason::Pressure));
        let mut dropped = Dropped::default();
        assert!(matches!(
            stack.take_one(at(0), &mut dropped, &()),
            Took::Read
        ));
        assert_eq!(dropped.0, [NetEvent::PressureCopy]);
        assert_eq!(stack.pool().free(), free);
        arrives(stack, b"formed", Holding::Copied(CopyReason::Format));
        assert!(matches!(
            stack.take_one(at(0), &mut dropped, &()),
            Took::Read
        ));
        assert_eq!(dropped.0, [NetEvent::PressureCopy], "a format copy is kept");
        assert_eq!(stack.pool().held(group), 1);
        let datagram = stack.receive(&socket).unwrap().unwrap();
        assert_eq!(datagram.block.octets(), b"formed");
        assert!(stack.pool().loans().released(), "a copy takes no loan");
        stack.pool().give(datagram.block);
    });
}

#[test]
fn a_copying_socket_copies_a_lendable_frame_without_a_loan() {
    lending!(|stack| {
        stack.pool().loans().pool(4, 1).unwrap();
        let group = stack.pool().group(4).unwrap();
        let socket = stack.open_in(PORT, best_effort(), group).unwrap();
        arrives(stack, b"copied", Holding::Lendable);
        let mut dropped = Dropped::default();
        assert!(matches!(
            stack.take_one(at(0), &mut dropped, &()),
            Took::Read
        ));
        let datagram = stack.receive(&socket).unwrap().unwrap();
        assert_eq!(datagram.block.octets(), b"copied");
        assert!(stack.pool().loans().released());
        stack.pool().give(datagram.block);
    });
}

#[test]
fn a_credit_the_account_cannot_hold_refuses_the_socket_and_changes_nothing() {
    lending!(|stack| {
        stack.pool().loans().pool(4, 3).unwrap();
        let group = stack.pool().group(4).unwrap();
        assert_eq!(
            stack.open_lending(PORT, best_effort(), group, 2).err(),
            Some(OpenRefused::Loans(LoansRefused::Pool { room: 1 }))
        );
        // The port stayed free, and the account lends nothing.
        let socket = stack.open_in(PORT, best_effort(), group).unwrap();
        arrives(stack, b"copied", Holding::Lendable);
        let mut dropped = Dropped::default();
        assert!(matches!(
            stack.take_one(at(0), &mut dropped, &()),
            Took::Read
        ));
        assert!(stack.receive(&socket).unwrap().is_some());
    });
}

#[test]
fn a_close_takes_the_credit_back_and_a_held_loan_counts_until_it_returns() {
    lending!(|stack| {
        stack.pool().loans().pool(2, 0).unwrap();
        let group = stack.pool().group(4).unwrap();
        let socket = stack.open_lending(PORT, best_effort(), group, 2).unwrap();
        arrives(stack, b"held", Holding::Lendable);
        let mut dropped = Dropped::default();
        let Took::Lent(held) = stack.take_one(at(0), &mut dropped, &()) else {
            panic!("lent");
        };
        let _closing = stack.close(socket).unwrap();
        assert_eq!(
            stack.pool().loans().outstanding(),
            1,
            "a close returns nothing"
        );
        // Another lending socket finds the held buffer counted against it.
        assert_eq!(
            stack.open_lending(PORT + 1, best_effort(), group, 2).err(),
            Some(OpenRefused::Loans(LoansRefused::Pool { room: 1 }))
        );
        stack.pool().loans().give_back(held.loan);
        assert!(
            stack
                .open_lending(PORT + 1, best_effort(), group, 2)
                .is_ok()
        );
        assert!(stack.pool().loans().released());
    });
}

#[test]
fn a_lane_that_takes_no_datagram_ends_the_loan_at_once() {
    lending!(|stack| {
        stack.pool().loans().pool(4, 1).unwrap();
        let group = stack.pool().group(4).unwrap();
        let _socket = stack.open_lending(PORT, best_effort(), group, 1).unwrap();
        arrives(stack, b"kept", Holding::Lendable);
        arrives(stack, b"next", Holding::Lendable);
        let mut dropped = Dropped::default();
        let mut offered = 0;
        let intake = stack.take_frames(at(0), 8, &mut dropped, &(), |datagram| {
            offered += 1;
            let crate::LentDatagram { frame, loan, .. } = datagram;
            drop(frame);
            Err(crate::Unlent::LaneFull(Some(loan)))
        });
        assert_eq!((intake.batch.taken, intake.lent, intake.unlent), (2, 0, 2));
        assert_eq!(offered, 2, "the refusal of one stops not the batch");
        assert!(stack.pool().loans().released(), "each loan back at once");
    });
}
