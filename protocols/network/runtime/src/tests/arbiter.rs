//! Two sockets of one group share the free blocks a turn grants: the cursor
//! moves on after each grant, so one socket's large request takes no whole
//! turn. Only the public API, so it reads the same on the arbiter before
//! reservations were batched.

use super::*;

const POOL: usize = 8;

type Pair<B> =
    Service<'static, 'static, B, SIZE, 2, CREDITS, RECEIVED, 2, POOL, DEPTH, RECEIVED, WirePort>;

/// A block granted to `socket` and not yet taken, if any; with blocks still
/// asked for, it asks for nothing more.
fn granted<B>(socket: &mut TestSocket<B>) -> Option<Block<'static, B, SIZE>> {
    let cx = &mut Context::from_waker(Waker::noop());
    match pin!(socket.reserve()).poll(cx) {
        Poll::Ready(block) => Some(block.unwrap()),
        Poll::Pending => None,
    }
}

#[test]
fn sockets_of_one_group_share_a_turn_s_blocks_whatever_each_asks() {
    brand!(Net);
    let memory = Box::leak(Box::new([[0; SIZE]; POOL]));
    let (pool, brand) = claim::<Net, SIZE, POOL>(memory).unwrap();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(brand, interface, pool, WirePort::default());
    let (_stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service: Pair<Net> = Service::new(owner, MemoryPolicy::KEEP, stopping);
    let dscp = Dscp::default();
    let group = service.group(POOL).unwrap();
    let mut first = service
        .bind(5000, dscp, group, storage::<Net>())
        .ok()
        .unwrap();
    let mut second = service
        .bind(5001, dscp, group, storage::<Net>())
        .ok()
        .unwrap();
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut events = ();
    let (mut firsts, mut seconds) = (Vec::new(), Vec::new());
    {
        let mut run = pin!(service.run(clock, &mut events));
        // Eight free; in one turn the first asks for sixteen, the second for
        // one.
        first.reserve_ahead(16);
        assert!(first.try_reserve().unwrap().is_none());
        second.reserve_ahead(1);
        assert!(second.try_reserve().unwrap().is_none());
        let cx = &mut Context::from_waker(Waker::noop());
        assert!(run.as_mut().poll(cx).is_pending());
        firsts.extend(core::iter::from_fn(|| granted(&mut first)));
        seconds.extend(core::iter::from_fn(|| granted(&mut second)));
    }
    assert_eq!(seconds.len(), 1, "the second's one block, in the same turn");
    assert!(!firsts.is_empty());
    assert!(firsts.len() + seconds.len() <= POOL);
    let mut ended = service.retire();
    for block in firsts.into_iter().chain(seconds) {
        ended.retired.pool.give(block);
    }
    while let Some(block) = first.reclaim().or_else(|| second.reclaim()) {
        ended.retired.pool.give(block);
    }
    assert_eq!(ended.retired.pool.free(), POOL);
    assert_eq!(ended.retired.pool.held(group), 0);
}
