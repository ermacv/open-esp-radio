extern crate std;

use super::*;
use std::vec::Vec;

const BE: WmmUserPriority = WmmUserPriority::UP0;
/// Best effort's other TID.
const BE3: WmmUserPriority = WmmUserPriority::UP3;
const VI: WmmUserPriority = WmmUserPriority::UP5;
const VO: WmmUserPriority = WmmUserPriority::UP6;

type Station = TxQueues<u32, 16, 1>;
type Access = TxQueues<u32, 32, 3>;

fn q(tid: WmmUserPriority) -> QueueId {
    QueueId::of_tid(tid)
}

fn of(peer: u8, tid: WmmUserPriority) -> QueueId {
    QueueId::new(peer, tid).unwrap()
}

fn msdu(tag: u32, priority: WmmUserPriority) -> OutboundMsdu<u32> {
    OutboundMsdu {
        memory: tag,
        length: 60,
        priority,
    }
}

fn estimated(micros: u32) -> TxEstimate {
    TxEstimate::Estimated { micros }
}

/// Each queue keeps its order; nothing of one is taken from another.
#[test]
fn each_queue_keeps_its_order_and_its_own_msdus() {
    let mut queues = Station::new();
    for (tag, tid) in [(1, BE), (2, VO), (3, BE), (4, VO)] {
        assert!(queues.push(q(tid), msdu(tag, tid)).is_ok());
    }
    assert_eq!(queues.pop(q(BE)).map(|m| m.memory), Some(1));
    assert_eq!(queues.pop(q(BE)).map(|m| m.memory), Some(3));
    assert_eq!(queues.pop(q(BE)).map(|m| m.memory), None);
    assert_eq!(queues.pop(q(VO)).map(|m| m.memory), Some(2));
    assert!(queues.push_front(q(VO), msdu(0, VO)).is_ok());
    assert_eq!(
        queues.pop(q(VO)).map(|m| m.memory),
        Some(0),
        "back at its head"
    );
    assert_eq!(queues.pop(q(VO)).map(|m| m.memory), Some(4));
    assert!(queues.is_empty());
}

/// A TID blocked by its window holds only its own queue: the other TID of
/// the same access category is chosen while it waits.
#[test]
fn a_tid_blocked_by_its_window_does_not_hold_another_tid_of_its_category() {
    let mut queues = Station::new();
    assert!(queues.push(q(BE), msdu(1, BE)).is_ok());
    assert!(queues.push(q(BE3), msdu(2, BE3)).is_ok());
    queues.block(q(BE));
    assert_eq!(queues.select(None), Some(q(BE3)));
    queues.block(q(BE3));
    assert_eq!(queues.select(None), None, "both wait for their windows");
    assert!(!queues.has_ready());
    queues.unblock();
    assert!(queues.select(None).is_some());
}

/// Voice goes before a best-effort backlog, and a held queue is passed
/// over.
#[test]
fn voice_goes_before_a_best_effort_backlog_and_a_held_queue_is_passed_over() {
    let mut queues = Station::new();
    for tag in 0..8 {
        assert!(queues.push(q(BE), msdu(tag, BE)).is_ok());
    }
    assert_eq!(queues.select(None), Some(q(BE)));
    assert!(queues.push(q(VI), msdu(100, VI)).is_ok());
    assert_eq!(queues.select(None), Some(q(VI)));
    assert!(queues.push(q(VO), msdu(101, VO)).is_ok());
    assert_eq!(
        queues.select(None),
        Some(q(VO)),
        "voice waits for no backlog"
    );
    assert_eq!(
        queues.select(Some(q(VO))),
        Some(q(VI)),
        "the held queue waits"
    );
}

/// Within one access category the queue that had the least air goes next,
/// each attempt charged once; one of unknown cost is charged the dearest
/// known, never nothing; a queue that starts holding MSDUs takes no credit
/// for its idle time.
#[test]
fn within_a_category_the_least_served_queue_goes_next() {
    let mut queues = Station::new();
    for tag in 0..4 {
        assert!(queues.push(q(BE), msdu(tag, BE)).is_ok());
        assert!(queues.push(q(BE3), msdu(10 + tag, BE3)).is_ok());
    }
    let mut served = Vec::new();
    for _ in 0..4 {
        let queue = queues.select(None).unwrap();
        served.push(queue);
        let _msdu = queues.pop(queue);
        queues.charge(queue, estimated(1000));
    }
    assert_eq!(
        served.iter().filter(|queue| **queue == q(BE)).count(),
        2,
        "{served:?}"
    );
    queues.charge(q(BE), estimated(3000));
    queues.charge(q(BE), TxEstimate::Unknown);
    assert_eq!(
        queues.select(None),
        Some(q(BE3)),
        "BE took 3000 + 3000 more"
    );
    while queues.pop(q(BE3)).is_some() {}
    queues.charge(q(BE), estimated(50_000));
    while queues.pop(q(BE)).is_some() {}
    assert!(queues.push(q(BE), msdu(1, BE)).is_ok());
    assert!(queues.push(q(BE3), msdu(2, BE3)).is_ok());
    let first = queues.select(None).unwrap();
    let _msdu = queues.pop(first);
    queues.charge(first, estimated(1000));
    assert_ne!(queues.select(None), Some(first), "the other goes next");
}

/// Two peers share the air by what their attempts cost, and a peer's
/// several TIDs earn it no more than one: the peer with one TID goes as
/// often as the one with two.
#[test]
fn peers_share_the_air_and_several_tids_earn_a_peer_no_more() {
    let mut queues = Access::new();
    for tag in 0..8 {
        assert!(queues.push(of(0, BE), msdu(tag, BE)).is_ok());
        assert!(queues.push(of(0, BE3), msdu(100 + tag, BE3)).is_ok());
        assert!(queues.push(of(1, BE), msdu(200 + tag, BE)).is_ok());
    }
    let mut turns = [0_u32; 2];
    for _ in 0..12 {
        let queue = queues.select(None).unwrap();
        turns[usize::from(queue.peer())] += 1;
        let _msdu = queues.pop(queue);
        queues.charge(queue, estimated(1000));
    }
    assert_eq!(turns, [6, 6], "each peer half the air");
    // A dearer peer gets fewer attempts for the same air.
    let mut queues = Access::new();
    for tag in 0..8 {
        assert!(queues.push(of(0, BE), msdu(tag, BE)).is_ok());
        assert!(queues.push(of(1, BE), msdu(100 + tag, BE)).is_ok());
    }
    let mut air = [0_u32; 2];
    for _ in 0..9 {
        let queue = queues.select(None).unwrap();
        let cost = if queue.peer() == 0 { 2000 } else { 1000 };
        air[usize::from(queue.peer())] += cost;
        let _msdu = queues.pop(queue);
        queues.charge(queue, estimated(cost));
    }
    assert!(air[0].abs_diff(air[1]) <= 2000, "{air:?}");
}

/// A full pool refuses with the MSDU back; a stop takes every MSDU, once;
/// a queue past the peers is refused.
#[test]
fn a_full_pool_refuses_and_a_drain_returns_every_msdu_once() {
    let mut queues = TxQueues::<u32, 4, 1>::new();
    for tag in 0..4 {
        let tid = WmmUserPriority::new(tag as u8 % 8).unwrap();
        assert!(queues.push(q(tid), msdu(tag, BE)).is_ok());
    }
    assert!(!queues.has_room());
    assert_eq!(
        queues.push(q(BE), msdu(9, BE)).err().map(|m| m.memory),
        Some(9)
    );
    let mut back = Vec::new();
    queues.drain(|msdu| back.push(msdu.memory));
    back.sort_unstable();
    assert_eq!(back, [0, 1, 2, 3]);
    assert!(queues.is_empty() && queues.has_room());
    assert_eq!(
        queues.push(of(1, BE), msdu(7, BE)).err().map(|m| m.memory),
        Some(7),
        "one peer only"
    );
    for tag in 0..4 {
        assert!(queues.push(q(VO), msdu(tag, VO)).is_ok());
    }
    let mut any = Vec::new();
    while let Some(msdu) = queues.pop_any() {
        any.push(msdu.memory);
    }
    assert_eq!(any, [0, 1, 2, 3]);
}
