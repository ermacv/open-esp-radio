extern crate std;

use std::boxed::Box;
use std::cell::{Cell, RefCell};

use oer_protocols_network_core::{
    Block, BlockPlace, Brand, NetworkClaim, Pool, claim, with_local_network,
};

use super::{ReturnEnd, ReturnLane, ReturnRefusal, Returner};

const SIZE: usize = 8;
const BLOCKS: usize = 3;

type Lane<'n, B> = ReturnLane<'n, B, SIZE, BLOCKS>;
type Blocks<'n, B> = Pool<'n, B, SIZE, BLOCKS>;
type Sides<'n, B> = (
    Blocks<'n, B>,
    Returner<'n, 'n, B, SIZE, BLOCKS>,
    ReturnEnd<'n, 'n, B, SIZE, BLOCKS>,
);

fn sides<'n, B: Brand + 'n>(right: NetworkClaim<B>) -> Sides<'n, B> {
    let memory = Box::leak(Box::new([const { BlockPlace::<SIZE>::new() }; BLOCKS]));
    let (pool, _) = claim(right, memory);
    let lane: &mut Lane<'n, B> = Box::leak(Box::new(ReturnLane::new()));
    let (returner, end) = lane.split();
    (pool, returner, end)
}

fn marked<'n, B: 'n>(pool: &mut Blocks<'n, B>, mark: u8) -> Block<'n, B, SIZE> {
    let mut block = pool.take().unwrap();
    block.fill(&[mark]).unwrap();
    block
}

#[test]
fn a_block_given_before_the_close_comes_home_and_one_after_is_refused() {
    with_local_network(|right| {
        let (mut pool, returner, end) = sides(right);
        let first = marked(&mut pool, 1);
        let second = marked(&mut pool, 2);
        assert!(returner.give(first).is_ok());
        end.close(&mut pool);
        assert_eq!(pool.outstanding(), 1, "the first came home");
        let refused = returner.give(second).err().unwrap();
        assert_eq!(refused.reason, ReturnRefusal::Closed);
        assert_eq!(refused.block.octets(), [2]);
    });
}

#[test]
fn a_block_in_the_lane_when_its_end_is_dropped_stays_within_reach() {
    with_local_network(|right| {
        let (mut pool, returner, end) = sides(right);
        let block = marked(&mut pool, 1);
        assert!(returner.reclaim().is_none());
        returner.give(block).ok().unwrap();
        assert!(returner.reclaim().is_none(), "open, it is the end's");
        drop(end);
        let block = returner.reclaim().unwrap();
        assert_eq!(block.octets(), [1]);
        assert!(returner.reclaim().is_none());
        pool.give(block);
    });
}

#[test]
fn moving_the_end_closes_nothing() {
    with_local_network(|right| {
        let (mut pool, returner, end) = sides(right);
        let block = marked(&mut pool, 1);
        let moved = Some(end);
        let end = moved.unwrap();
        returner.give(block).ok().unwrap();
        assert!(end.take_home(&mut pool));
        assert_eq!(pool.outstanding(), 0);
    });
}

#[test]
fn a_close_between_the_check_and_the_store_refuses_the_block() {
    with_local_network(|right| {
        let (mut pool, returner, end) = sides(right);
        let block = marked(&mut pool, 1);
        let pool = RefCell::new(pool);
        let end = Cell::new(Some(end));
        let closed = returner.lane.give_at(
            block,
            || end.take().unwrap().close(&mut pool.borrow_mut()),
            || {},
        );
        let refused = closed.err().unwrap();
        assert_eq!(refused.reason, ReturnRefusal::Closed);
        assert_eq!(refused.block.octets(), [1]);
        assert_eq!(pool.borrow().outstanding(), 1, "it is with its caller");
    });
}

#[test]
fn a_close_after_the_store_that_found_the_block_accepts_it() {
    with_local_network(|right| {
        let (mut pool, returner, end) = sides(right);
        let block = marked(&mut pool, 1);
        let pool = RefCell::new(pool);
        let end = Cell::new(Some(end));
        let given = returner.lane.give_at(
            block,
            || {},
            || end.take().unwrap().close(&mut pool.borrow_mut()),
        );
        assert!(given.is_ok(), "the close took it home");
        assert_eq!(pool.borrow().outstanding(), 0);
    });
}

#[test]
fn a_dropped_end_after_the_store_refuses_a_block_no_one_read() {
    with_local_network(|right| {
        let (mut pool, returner, end) = sides(right);
        let block = marked(&mut pool, 1);
        let end = Cell::new(Some(end));
        let given = returner.lane.give_at(block, || {}, || drop(end.take()));
        let refused = given.err().unwrap();
        assert_eq!(refused.reason, ReturnRefusal::Closed);
        assert_eq!(refused.block.octets(), [1]);
        assert!(returner.reclaim().is_none(), "it is not in two places");
        pool.give(refused.block);
    });
}

#[test]
fn a_give_takes_back_only_its_own_block() {
    with_local_network(|right| {
        let (mut pool, returner, end) = sides(right);
        let mine = marked(&mut pool, 1);
        let later = marked(&mut pool, 2);
        let pool = RefCell::new(pool);
        let end = Cell::new(Some(end));
        let returned = returner.lane.give_at(
            mine,
            || {},
            || {
                // Mine goes home with the close, and a give caught past its check
                // fills the same slot.
                let closer = end.take().unwrap();
                closer.lane.drain(&mut pool.borrow_mut());
                let _second = closer.lane.give_at(later, || {}, || {});
                core::mem::forget(closer);
                returner.lane.close();
            },
        );
        assert!(returned.is_ok(), "its own block had gone home");
        assert_eq!(returner.reclaim().unwrap().octets(), [2]);
    });
}

#[test]
fn copies_of_a_returner_share_the_lane() {
    with_local_network(|right| {
        let (mut pool, returner, end) = sides(right);
        let (one, two) = (marked(&mut pool, 1), marked(&mut pool, 2));
        let copy = returner;
        returner.give(one).ok().unwrap();
        copy.give(two).ok().unwrap();
        assert!(end.take_home(&mut pool));
        assert_eq!(pool.outstanding(), 0);
    });
}

#[test]
fn the_lane_holds_every_block_of_its_pool() {
    with_local_network(|right| {
        let (mut pool, returner, end) = sides(right);
        let blocks = [1, 2, 3].map(|mark| marked(&mut pool, mark));
        for block in blocks {
            returner.give(block).ok().unwrap();
        }
        end.close(&mut pool);
        assert_eq!(pool.outstanding(), 0);
    });
}

#[test]
fn a_slot_held_by_a_lower_context_refuses_for_now_and_keeps_the_block() {
    with_local_network(|right| {
        let (mut pool, returner, end) = sides(right);
        let [one, two, three] = [1, 2, 3].map(|mark| marked(&mut pool, mark));
        returner.give(one).ok().unwrap();
        returner.give(two).ok().unwrap();
        let last = returner.lane.slots.last().unwrap();
        let three = last
            .lend(|_| returner.give(three).err().unwrap())
            .ok()
            .unwrap();
        assert_eq!(three.reason, ReturnRefusal::Busy);
        returner.give(three.block).ok().unwrap();
        assert!(end.take_home(&mut pool));
        assert_eq!(pool.outstanding(), 0);
    });
}
