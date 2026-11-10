extern crate std;

use std::vec::Vec;

use oer_protocols_network_wire::ip::Dscp;

use super::Turns;
use crate::frame::{Class, Ready};

fn ready(class: Class, flow: Option<u8>) -> Ready {
    Ready {
        class,
        length: 100,
        peer: [2, 0, 0, 0, 0, 1],
        flow,
        dscp: Dscp::default(),
    }
}

/// The works a choice that sees them all sends, one at a time: control first
/// as listed, then the flow nearest in turn after `last`, its first listed
/// work, each data frame moving `last` to its flow. This is the choice a plan
/// stands for, written apart from it.
fn reference(works: &[Ready], mut last: Option<u8>) -> Vec<usize> {
    let mut left: Vec<usize> = (0..works.len()).collect();
    let mut sent = Vec::new();
    while !left.is_empty() {
        let (at, _) = left
            .iter()
            .enumerate()
            .min_by_key(|(_, index)| {
                let work = works[**index];
                let flow = work.flow.unwrap_or(0);
                let distance = last.map_or(flow, |last| flow.wrapping_sub(last).wrapping_sub(1));
                (work.class != Class::Control, distance)
            })
            .unwrap();
        let index = left.remove(at);
        if works[index].class == Class::Data {
            last = works[index].flow;
        }
        sent.push(index);
    }
    sent
}

/// Lists of ready work: control here and there, data of a few flows in any
/// order, as a pseudorandom sequence fixed by `seed` gives them.
fn listing(seed: u64, length: usize, flows: u8) -> Vec<Ready> {
    let mut state = seed;
    (0..length)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let pick = (state >> 33) as u8;
            if pick.is_multiple_of(7) {
                ready(Class::Control, None)
            } else {
                ready(Class::Data, Some(pick % flows))
            }
        })
        .collect()
}

fn planned<const Q: usize>(works: &[Ready], mut turns: Turns<usize, Q>) -> (Vec<usize>, bool) {
    for (index, work) in works.iter().enumerate() {
        turns.offer(work, index);
    }
    let more = turns.more();
    let mut sent = Vec::new();
    // Empty exactly when a take finds nothing.
    while !turns.is_empty() {
        sent.push(turns.take().expect("a plan that is not empty holds a work"));
    }
    assert!(turns.take().is_none(), "an empty plan holds no work");
    (sent, more)
}

fn check<const Q: usize>(works: &[Ready], last: Option<u8>) {
    let expected = reference(works, last);
    let (sent, more) = planned(works, Turns::<usize, Q>::after(last));
    let length = expected.len().min(Q);
    assert_eq!(sent, expected[..length], "{works:?} after {last:?}");
    assert_eq!(more, works.len() > Q, "more is ready than held");
}

/// A plan holds the first works of the choice it stands for, in its order,
/// for any list, any last served flow and a plan shorter than the list:
/// control first as listed, the flows in turn after the last, one work each
/// a round.
#[test]
fn a_plan_holds_the_first_works_of_its_choice_in_order() {
    for seed in 0..400 {
        let flows = 1 + (seed % 9) as u8;
        let length = (seed % 23) as usize;
        let works = listing(seed, length, flows);
        for last in [None, Some(0), Some(1), Some(4), Some(255)] {
            check::<8>(&works, last);
            check::<3>(&works, last);
            check::<1>(&works, last);
        }
    }
}

/// More flows than the plan holds: the nearest in turn stay, whatever order
/// they come in, and a farther one that came first goes with its works.
#[test]
fn flows_past_the_plan_give_way_to_nearer_ones() {
    let works: Vec<Ready> = [9, 8, 7, 1, 1, 2, 3, 0]
        .into_iter()
        .map(|flow| ready(Class::Data, Some(flow)))
        .collect();
    check::<3>(&works, None);
    let (sent, more) = planned(&works, Turns::<usize, 3>::after(None));
    assert_eq!(sent, [7, 3, 5], "flows 0, 1 and 2 first");
    assert!(more);
}

/// The list's own turn starts at its first data work's flow: a network that
/// lists data from the socket whose turn it is, and passes the turn on at
/// each frame, sends its sockets in turn from there.
#[test]
fn a_listed_turn_starts_at_the_first_data_flow() {
    let works: Vec<Ready> = [
        ready(Class::Control, None),
        ready(Class::Data, Some(2)),
        ready(Class::Data, Some(2)),
        ready(Class::Data, Some(3)),
        ready(Class::Data, Some(0)),
        ready(Class::Data, Some(0)),
    ]
    .into();
    let (sent, more) = planned(&works, Turns::<usize, 8>::listed());
    assert_eq!(sent, [0, 1, 3, 4, 2, 5]);
    assert!(!more);
    assert_eq!(sent, reference(&works, Some(1)));
}

/// A plan started again in place holds what a fresh one holds: nothing of
/// the works offered before, half taken or not, and of `more`, stays.
#[test]
fn a_plan_started_again_holds_what_a_fresh_one_holds() {
    for seed in 0..200 {
        let flows = 1 + (seed % 9) as u8;
        let before = listing(seed, (seed % 23) as usize, flows);
        let works = listing(seed.wrapping_add(1_000), ((seed + 7) % 23) as usize, flows);
        for last in [None, Some(0), Some(3), Some(255)] {
            let mut reused = Turns::<usize, 8>::after(Some(5));
            for (index, work) in before.iter().enumerate() {
                reused.offer(work, index + 100);
            }
            // Some of it taken, the rest left behind.
            let _ = reused.take();
            reused.restart(last);
            assert_eq!(
                planned(&works, reused),
                planned(&works, Turns::<usize, 8>::after(last)),
                "seed {seed}, after {last:?}"
            );
        }
        let mut relisted = Turns::<usize, 8>::after(Some(2));
        for (index, work) in before.iter().enumerate() {
            relisted.offer(work, index + 100);
        }
        relisted.relist();
        assert_eq!(
            planned(&works, relisted),
            planned(&works, Turns::<usize, 8>::listed()),
            "seed {seed}, listed"
        );
    }
}
