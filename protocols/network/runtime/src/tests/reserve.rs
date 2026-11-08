//! A socket asks for its blocks in one request: what that request owes, and
//! what its cancellation, a close and a retirement leave. Each test keeps its
//! own expected count of where every block is, and checks it against what
//! the sockets and the pool's holder see.

use core::convert::Infallible;

use oer_portable_events::EventSink;
use oer_protocols_network_core::NetEvent;

use super::*;
use crate::{RequestKind, ServiceEvent};

/// The pool of these tests.
const POOL: usize = 8;

type Svc<B> = Service<
    'static,
    'static,
    B,
    SIZE,
    3,
    CREDITS,
    RECEIVED,
    2,
    POOL,
    DEPTH,
    RECEIVED,
    WirePort,
    Direct,
>;
type Sock<B> = TestSocket<B>;
type Held<B> = Vec<Block<'static, B, SIZE>>;
type Ended3<B> = Ended<
    'static,
    'static,
    B,
    SIZE,
    3,
    CREDITS,
    POOL,
    DEPTH,
    RECEIVED,
    WirePort,
    Direct,
    Infallible,
>;

/// The blocks each reservation the service took asked for, in order.
#[derive(Default)]
struct Asked(Vec<u8>);

impl EventSink<ServiceEvent> for Asked {
    fn event(&mut self, event: ServiceEvent) {
        if let ServiceEvent::Took(RequestKind::Reserve { blocks }) = event {
            self.0.push(blocks);
        }
    }
}

impl EventSink<NetEvent> for Asked {
    fn event(&mut self, _: NetEvent) {}
}

/// A holder in the shared group, and two sockets in groups of their own,
/// limited to `limits`.
struct Three<B: 'static> {
    service: Svc<B>,
    holder: Sock<B>,
    first: Sock<B>,
    second: Sock<B>,
    groups: [Group<B>; 2],
    clock: &'static VirtualClock,
}

fn three<B: 'static>(limits: [usize; 2]) -> Three<B> {
    let memory = Box::leak(Box::new([[0; SIZE]; POOL]));
    let (pool, brand) = claim::<B, SIZE, POOL>(memory).unwrap();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(brand, interface, pool, WirePort::default());
    let (_stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::new(owner, Direct, MemoryPolicy::KEEP, stopping);
    let dscp = Dscp::default();
    let holder = service
        .bind(5000, dscp, Group::shared(), storage::<B>())
        .ok()
        .unwrap();
    let groups = limits.map(|limit| service.group(limit).unwrap());
    let first = service
        .bind(5001, dscp, groups[0], storage::<B>())
        .ok()
        .unwrap();
    let second = service
        .bind(5002, dscp, groups[1], storage::<B>())
        .ok()
        .unwrap();
    Three {
        service,
        holder,
        first,
        second,
        groups,
        clock: Box::leak(Box::new(VirtualClock::new())),
    }
}

/// Poll `run` once: one turn, when it has work.
fn pass<R: Future>(run: &mut core::pin::Pin<&mut R>) {
    let cx = &mut Context::from_waker(Waker::noop());
    assert!(run.as_mut().poll(cx).is_pending());
}

/// Take `count` blocks into `socket`'s application, one a turn, leaving
/// nothing asked.
fn hold<B, R: Future>(
    socket: &mut Sock<B>,
    run: &mut core::pin::Pin<&mut R>,
    count: usize,
) -> Held<B> {
    // One at a time: a look-ahead of one asks only when it finds none.
    socket.reserve_ahead(1);
    let mut held = Vec::new();
    while held.len() < count {
        match socket.try_reserve().unwrap() {
            Some(block) => held.push(block),
            None => pass(run),
        }
    }
    held
}

/// A block granted to `socket` and not yet taken, if any. With blocks still
/// asked for, a reserve that finds none asks for nothing more.
fn granted<B>(socket: &mut Sock<B>) -> Option<Block<'static, B, SIZE>> {
    let cx = &mut Context::from_waker(Waker::noop());
    match pin!(socket.reserve()).poll(cx) {
        Poll::Ready(block) => Some(block.unwrap()),
        Poll::Pending => None,
    }
}

/// What `hold` asks for: one block a request.
fn ones(count: usize) -> Vec<u8> {
    std::vec![1; count]
}

/// After a retirement: everything in hands goes home, and the pool is whole,
/// with no group charged.
fn whole<B>(ended: &mut Ended3<B>, groups: [Group<B>; 2], hands: [Held<B>; 3]) {
    for block in hands.into_iter().flatten() {
        ended.retired.pool.give(block);
    }
    let pool = &ended.retired.pool;
    assert_eq!((pool.free(), pool.outstanding()), (POOL, 0));
    for group in groups.into_iter().chain([Group::shared()]) {
        assert_eq!(pool.held(group), 0);
    }
}

#[test]
fn a_reservation_is_granted_block_by_block_as_blocks_come_home() {
    brand!(Net);
    let Three {
        mut service,
        mut holder,
        mut first,
        groups,
        clock,
        ..
    } = three::<Net>([POOL, POOL]);
    let mut asked = Asked::default();
    let mut taken = Vec::new();
    let mut held;
    {
        let mut run = pin!(service.run(clock, &mut asked));
        held = hold(&mut holder, &mut run, POOL - 1);
        // One free, four asked for in one request.
        first.reserve_ahead(4);
        assert!(first.try_reserve().unwrap().is_none());
        pass(&mut run);
        taken.push(granted(&mut first).expect("the one free block"));
        assert!(granted(&mut first).is_none(), "none other is free");
        // Each block that comes home goes to the socket. Every second block
        // it takes leaves half its look-ahead of four out, and the take asks
        // for those two in one request.
        for _ in 0..3 {
            holder.release(held.pop().unwrap()).ok().unwrap();
            pass(&mut run);
            taken.push(granted(&mut first).expect("a block it is owed"));
        }
        // The next block home goes to what it asks for still: it stays in
        // the lane, not taken.
        holder.release(held.pop().unwrap()).ok().unwrap();
        pass(&mut run);
    }
    let mut expected = ones(POOL - 1);
    expected.extend([4, 2, 2]);
    assert_eq!(
        asked.0, expected,
        "the holder's, the socket's, and the two its takes renewed"
    );
    let mut ended = service.retire();
    assert_eq!(ended.retired.pool.free(), 0);
    assert_eq!(ended.retired.pool.held(groups[0]), 5);
    let reclaimed: Vec<_> = core::iter::from_fn(|| first.reclaim()).collect();
    assert_eq!(reclaimed.len(), 1, "the one granted and not taken");
    whole(&mut ended, groups, [held, taken, reclaimed]);
}

#[test]
fn a_cancelled_wait_keeps_what_its_request_is_granted() {
    brand!(Net);
    let Three {
        mut service,
        mut holder,
        mut first,
        groups,
        clock,
        ..
    } = three::<Net>([POOL, POOL]);
    let mut asked = Asked::default();
    let mut taken = Vec::new();
    let mut held;
    {
        let mut run = pin!(service.run(clock, &mut asked));
        held = hold(&mut holder, &mut run, POOL);
        first.reserve_ahead(4);
        {
            let cx = &mut Context::from_waker(Waker::noop());
            let mut waiting = pin!(first.reserve());
            assert!(waiting.as_mut().poll(cx).is_pending());
        }
        // The wait is gone; its request stays.
        pass(&mut run);
        for _ in 0..2 {
            holder.release(held.pop().unwrap()).ok().unwrap();
        }
        pass(&mut run);
        // A new wait finds both, and asks for nothing again.
        taken.extend(granted(&mut first));
        taken.extend(granted(&mut first));
        assert!(granted(&mut first).is_none());
        pass(&mut run);
    }
    assert_eq!(taken.len(), 2);
    // One request survives its cancelled wait; the second take, leaving half
    // the look-ahead out, asks for those two.
    let mut expected = ones(POOL);
    expected.extend([4, 2]);
    assert_eq!(asked.0, expected);
    let mut ended = service.retire();
    assert_eq!(ended.retired.pool.free(), 0);
    assert_eq!(ended.retired.pool.held(groups[0]), 2);
    assert!(first.reclaim().is_none(), "the two still owed never came");
    whole(&mut ended, groups, [held, taken, Vec::new()]);
}

#[test]
fn a_close_keeps_what_was_granted_and_ends_what_was_not() {
    brand!(Net);
    let Three {
        mut service,
        mut holder,
        mut first,
        groups,
        clock,
        ..
    } = three::<Net>([POOL, POOL]);
    let mut asked = Asked::default();
    let mut taken = Vec::new();
    let mut held;
    {
        let mut run = pin!(service.run(clock, &mut asked));
        held = hold(&mut holder, &mut run, POOL - 1);
        first.reserve_ahead(4);
        assert!(first.try_reserve().unwrap().is_none());
        pass(&mut run);
        // One granted and not taken, three owed; then the socket closes.
        let cx = &mut Context::from_waker(Waker::noop());
        {
            let mut closing = pin!(first.close());
            assert!(closing.as_mut().poll(cx).is_pending());
            pass(&mut run);
            for _ in 0..3 {
                holder.release(held.pop().unwrap()).ok().unwrap();
            }
            pass(&mut run);
            pass(&mut run);
            assert!(closing.as_mut().poll(cx).is_ready());
        }
        // The block granted before the close is the socket's still.
        taken.extend(first.try_reserve().unwrap());
        assert_eq!(first.try_reserve().err(), Some(SocketClosed));
    }
    assert_eq!(taken.len(), 1, "what was granted stays the socket's");
    let mut ended = service.retire();
    assert_eq!(
        ended.retired.pool.free(),
        3,
        "none is granted after the close"
    );
    assert_eq!(ended.retired.pool.held(groups[0]), 1);
    whole(&mut ended, groups, [held, taken, Vec::new()]);
}

#[test]
fn blocks_granted_while_the_close_waits_in_the_lane_stay_the_socket_s() {
    brand!(Net);
    let Three {
        mut service,
        mut holder,
        mut first,
        groups,
        clock,
        ..
    } = three::<Net>([POOL, POOL]);
    let mut asked = Asked::default();
    let mut taken = Vec::new();
    let mut held;
    {
        let mut run = pin!(service.run(clock, &mut asked));
        held = hold(&mut holder, &mut run, POOL - 3);
        let own = hold(&mut first, &mut run, 3);
        // Four asked for, three of its own blocks released, then a close:
        // the turn takes its budget of four, grants the three that came
        // home, and leaves the close for the next turn.
        first.reserve_ahead(4);
        assert!(first.try_reserve().unwrap().is_none());
        for block in own {
            first.release(block).ok().unwrap();
        }
        let cx = &mut Context::from_waker(Waker::noop());
        {
            let mut closing = pin!(first.close());
            assert!(closing.as_mut().poll(cx).is_pending());
            pass(&mut run);
            assert!(closing.as_mut().poll(cx).is_pending());
            pass(&mut run);
            for _ in 0..2 {
                holder.release(held.pop().unwrap()).ok().unwrap();
            }
            pass(&mut run);
            pass(&mut run);
            assert!(closing.as_mut().poll(cx).is_ready());
        }
        while let Ok(Some(block)) = first.try_reserve() {
            taken.push(block);
        }
        // Taking them asked for nothing: a turn finds no request.
        pass(&mut run);
    }
    let mut expected = ones(POOL);
    expected.push(4);
    assert_eq!(
        asked.0, expected,
        "a closing socket's takes ask for nothing"
    );
    assert_eq!(taken.len(), 3, "granted before the close was taken");
    let mut ended = service.retire();
    assert_eq!(ended.retired.pool.free(), 2, "none is granted after it");
    assert_eq!(ended.retired.pool.held(groups[0]), 3);
    whole(&mut ended, groups, [held, taken, Vec::new()]);
}

#[test]
fn a_retirement_ends_what_is_owed_and_every_granted_block_comes_back_once() {
    brand!(Net);
    let Three {
        mut service,
        mut holder,
        mut first,
        mut second,
        groups,
        clock,
    } = three::<Net>([POOL, POOL]);
    let mut asked = Asked::default();
    let held;
    let in_hand;
    {
        let mut run = pin!(service.run(clock, &mut asked));
        held = hold(&mut holder, &mut run, POOL - 2);
        in_hand = hold(&mut first, &mut run, 1);
        // One free, four asked for: one granted and left in the lane.
        first.reserve_ahead(4);
        assert!(first.try_reserve().unwrap().is_none());
        pass(&mut run);
        // A request the service never takes.
        second.reserve_ahead(2);
        assert!(second.try_reserve().unwrap().is_none());
    }
    let mut expected = ones(POOL - 1);
    expected.push(4);
    assert_eq!(asked.0, expected, "the second's request was never taken");
    let mut ended = service.retire();
    assert_eq!(ended.retired.pool.outstanding(), POOL);
    // The block granted and not taken comes back once; nothing else does.
    let reclaimed: Vec<_> = core::iter::from_fn(|| first.reclaim()).collect();
    assert_eq!(reclaimed.len(), 1);
    assert!(
        second.reclaim().is_none(),
        "no block for a request never taken"
    );
    assert_eq!(first.try_reserve().err(), Some(SocketClosed));
    // What the applications hold, they hand to the pool's holder.
    let mut hands = Vec::new();
    for block in held {
        hands.push(holder.release(block).err().unwrap().block);
    }
    whole(&mut ended, groups, [hands, in_hand, reclaimed]);
}

#[test]
fn a_reservation_past_its_group_s_limit_waits_with_blocks_free() {
    brand!(Net);
    let Three {
        mut service,
        mut first,
        groups,
        clock,
        ..
    } = three::<Net>([2, POOL]);
    let mut asked = Asked::default();
    let mut taken = Vec::new();
    {
        let mut run = pin!(service.run(clock, &mut asked));
        first.reserve_ahead(4);
        assert!(first.try_reserve().unwrap().is_none());
        pass(&mut run);
        taken.extend(granted(&mut first));
        taken.extend(granted(&mut first));
        pass(&mut run);
        assert!(granted(&mut first).is_none(), "six free, its limit is two");
        first.release(taken.pop().unwrap()).ok().unwrap();
        pass(&mut run);
        taken.extend(granted(&mut first));
        assert!(granted(&mut first).is_none());
    }
    assert_eq!(taken.len(), 2);
    assert_eq!(asked.0, [4, 2], "the second take asks for the two it took");
    let mut ended = service.retire();
    assert_eq!(ended.retired.pool.held(groups[0]), 2);
    whole(&mut ended, groups, [taken, Vec::new(), Vec::new()]);
}

#[test]
fn two_groups_short_of_blocks_share_them_block_by_block_whatever_they_ask() {
    brand!(Net);
    let Three {
        mut service,
        mut holder,
        mut first,
        mut second,
        groups,
        clock,
    } = three::<Net>([POOL, POOL]);
    let mut asked = Asked::default();
    let mut hands = [Vec::new(), Vec::new()];
    let mut got = Vec::new();
    let mut held;
    {
        let mut run = pin!(service.run(clock, &mut asked));
        held = hold(&mut holder, &mut run, POOL);
        // The first asks for all it may at once, the second for one at a
        // time.
        first.reserve_ahead(RESERVE_AHEAD);
        assert!(first.try_reserve().unwrap().is_none());
        second.reserve_ahead(1);
        assert!(second.try_reserve().unwrap().is_none());
        pass(&mut run);
        // Blocks come home one a turn, then two.
        for home in [1, 1, 1, 1, 2, 2] {
            for _ in 0..home {
                holder.release(held.pop().unwrap()).ok().unwrap();
            }
            pass(&mut run);
            let mut round = [0, 0];
            for (index, socket) in [&mut first, &mut second].into_iter().enumerate() {
                while let Some(block) = granted(socket) {
                    round[index] += 1;
                    hands[index].push(block);
                }
            }
            got.push(round);
        }
    }
    // The group holding least first; on a tie, the next in turn.
    assert_eq!(
        got,
        [[1, 0], [0, 1], [1, 0], [0, 1], [1, 1], [1, 1]],
        "per block, as many as asked for or not"
    );
    assert_eq!(asked.0[..POOL], ones(POOL));
    assert_eq!(asked.0[POOL..POOL + 2], [16, 1]);
    assert!(asked.0[POOL + 2..].iter().all(|blocks| *blocks == 1));
    let mut ended = service.retire();
    let [first_hand, second_hand] = hands;
    whole(&mut ended, groups, [held, first_hand, second_hand]);
}

#[test]
fn a_refill_leaves_the_lane_s_last_place_to_a_send_or_a_release() {
    brand!(Net);
    let Three {
        mut service,
        mut first,
        groups,
        clock,
        ..
    } = three::<Net>([POOL, POOL]);
    let mut asked = Asked::default();
    let mut hand = Vec::new();
    {
        let mut run = pin!(service.run(clock, &mut asked));
        first.reserve_ahead(RESERVE_AHEAD);
        assert!(first.try_reserve().unwrap().is_none());
        pass(&mut run);
        // All eight of the pool came; seven taken leave less than half out.
        for _ in 0..POOL - 1 {
            hand.push(first.try_reserve().unwrap().unwrap());
        }
        // Four of the lane's five places taken: two sends and two releases.
        for _ in 0..CREDITS {
            drop(first.send(peer(), hand.pop().unwrap()).ok().unwrap());
        }
        for _ in 0..2 {
            first.release(hand.pop().unwrap()).ok().unwrap();
        }
        // The eighth take leaves half out, but the last place is not a
        // refill's: the release after it finds it.
        let last = first.try_reserve().unwrap().unwrap();
        first.release(last).ok().unwrap();
        pass(&mut run);
        pass(&mut run);
    }
    assert_eq!(asked.0, [16], "no refill took the last place");
    let mut ended = service.retire();
    let reclaimed: Vec<_> = core::iter::from_fn(|| first.reclaim()).collect();
    whole(&mut ended, groups, [hand, reclaimed, Vec::new()]);
}

#[test]
fn a_reservation_reports_the_blocks_it_asks_for() {
    brand!(Net);
    let Three {
        mut service,
        mut first,
        mut second,
        clock,
        ..
    } = three::<Net>([POOL, POOL]);
    let mut asked = Asked::default();
    {
        let mut run = pin!(service.run(clock, &mut asked));
        // What the look-ahead lacks, once none is ready: five.
        first.reserve_ahead(5);
        assert!(first.try_reserve().unwrap().is_none());
        pass(&mut run);
        // Taking them: the third take leaves three of five out, half
        // rounded up, and asks for those three.
        let mut taken = Vec::new();
        for _ in 0..5 {
            taken.push(first.try_reserve().unwrap().expect("one of the five"));
        }
        // At most the look-ahead.
        second.reserve_ahead(usize::MAX);
        assert!(second.try_reserve().unwrap().is_none());
        pass(&mut run);
    }
    assert_eq!(asked.0, [5, 3, 16]);
}
