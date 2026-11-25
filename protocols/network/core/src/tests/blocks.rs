//! A pool's blocks: each has its place while out and while free.

extern crate std;

use std::vec::Vec;

use super::{BLOCKS, Port, SIZE, Stack, at, best_effort, claimed, filled, interface, peer};
use crate::{Block, BlockTooShort, Group, GroupsFull, NetworkOwner, with_local_network};

#[test]
fn every_block_is_taken_once_and_given_back() {
    with_local_network(|right| {
        let (mut pool, _brand) = claimed(right);
        for round in 0..3_u8 {
            let mut held = Vec::new();
            for out in 0..BLOCKS {
                assert_eq!((pool.free(), pool.outstanding()), (BLOCKS - out, out));
                let mut block = pool.take().expect("one is free");
                assert!(block.octets().is_empty(), "it comes emptied");
                let mark = round * 16 + u8::try_from(out).unwrap();
                block.fill(&[mark]).unwrap();
                held.push((block, mark));
            }
            assert!(pool.take().is_none(), "none is left");
            assert_eq!((pool.free(), pool.outstanding()), (0, BLOCKS));
            // Each is a place of its own: none wrote over another.
            for (block, mark) in &held {
                assert_eq!(block.octets(), [*mark]);
            }
            for (block, _) in held {
                pool.give(block);
            }
            assert_eq!((pool.free(), pool.outstanding()), (BLOCKS, 0));
        }
    });
}

/// The order is stated: the last given back is the next taken.
#[test]
fn the_last_block_given_back_is_the_next_taken() {
    with_local_network(|right| {
        let (mut pool, _brand) = claimed(right);
        let place = |block: &mut Block<'_, _, SIZE>| block.space().as_ptr().addr();
        let mut first = pool.take().unwrap();
        let mut second = pool.take().unwrap();
        let (one, two) = (place(&mut first), place(&mut second));
        assert_ne!(one, two);
        pool.give(second);
        pool.give(first);
        assert_eq!(place(&mut pool.take().unwrap()), one);
        assert_eq!(place(&mut pool.take().unwrap()), two);
    });
}

#[test]
fn a_block_holds_up_to_its_whole_space() {
    with_local_network(|right| {
        let (mut pool, _brand) = claimed(right);
        let mut block = pool.take().unwrap();
        block.space().fill(7);
        block.set_length(SIZE).unwrap();
        assert_eq!(block.octets(), [7; SIZE]);
        assert_eq!(block.set_length(SIZE + 1), Err(BlockTooShort));
        assert_eq!(block.fill(&[1; SIZE + 1]), Err(BlockTooShort));
        assert_eq!(block.octets(), [7; SIZE], "a refusal changed nothing");
        pool.give(block);
        assert!(
            pool.take().unwrap().octets().is_empty(),
            "taken anew, empty"
        );
    });
}

#[test]
fn a_block_counts_toward_the_group_it_was_taken_for() {
    with_local_network(|right| {
        let (mut pool, _brand) = claimed(right);
        let group = pool.group(2).unwrap();
        assert_eq!(pool.heaviest(), None);
        let first = pool.take_as(group).unwrap();
        let second = pool.take_as(group).unwrap();
        assert!(pool.take_as(group).is_none(), "its limit");
        assert_eq!(pool.free(), BLOCKS - 2, "with blocks free");
        let shared = pool.take().unwrap();
        assert!(first.group() == group && shared.group() == Group::shared());
        assert_eq!((pool.held(group), pool.held(Group::shared())), (2, 1));
        assert_eq!(pool.heaviest().map(|(_, held)| held), Some(2));
        pool.give(second);
        assert_eq!((pool.held(group), pool.held(Group::shared())), (1, 1));
        // The place it left counts toward whoever takes it next.
        let third = pool.take().unwrap();
        assert!(third.group() == Group::shared());
        assert_eq!((pool.held(group), pool.held(Group::shared())), (1, 2));
        for block in [first, shared, third] {
            pool.give(block);
        }
        assert_eq!((pool.held(group), pool.held(Group::shared())), (0, 0));
        assert_eq!(pool.outstanding(), 0);
        while pool.group(1).is_ok() {}
        assert_eq!(pool.group(1), Err(GroupsFull));
    });
}

/// Blocks outlive a core: those of a retired one count as out for the next,
/// until they are given back or for good if they never are.
#[test]
fn a_retired_core_s_blocks_count_for_the_next_over_the_same_pool() {
    with_local_network(|right| {
        let (pool, brand) = claimed(right);
        let mut stack: Stack<_> = NetworkOwner::new(
            brand,
            interface([192, 0, 2, 1], 24, None),
            pool,
            Port::default(),
        );
        let socket = stack.open(5000, best_effort()).unwrap();
        let queued = filled(stack.pool(), b"queued");
        let _command = stack.send(at(0), &socket, peer(), queued).unwrap();
        let kept = filled(stack.pool(), b"kept");
        let lost = filled(stack.pool(), b"lost");
        let retired = stack.retire();
        assert_eq!(
            retired.pool.outstanding(),
            2,
            "the core's own came back, the holders' did not"
        );
        let mut next: Stack<_> = NetworkOwner::new(
            retired.brand,
            interface([192, 0, 2, 1], 24, None),
            retired.pool,
            Port::default(),
        );
        assert_eq!(
            (next.pool().free(), next.pool().outstanding()),
            (BLOCKS - 2, 2)
        );
        assert_eq!(kept.octets(), b"kept");
        next.pool().give(kept);
        drop(lost);
        assert_eq!(
            (next.pool().free(), next.pool().outstanding()),
            (BLOCKS - 1, 1)
        );
        let held: Vec<_> = core::iter::from_fn(|| next.pool().take())
            .take(BLOCKS + 1)
            .collect();
        assert_eq!(held.len(), BLOCKS - 1, "a dropped block is not free");
    });
}
