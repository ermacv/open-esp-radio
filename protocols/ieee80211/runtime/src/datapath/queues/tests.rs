extern crate std;

use super::*;
use std::vec::Vec;

const BE: WmmUserPriority = WmmUserPriority::UP0;
/// Best effort's other TID.
const BE3: WmmUserPriority = WmmUserPriority::UP3;
const VI: WmmUserPriority = WmmUserPriority::UP5;
const VO: WmmUserPriority = WmmUserPriority::UP6;

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

/// Each TID keeps its own order; nothing of one is taken from another.
#[test]
fn each_tid_keeps_its_order_and_its_own_msdus() {
    let mut queues = TxQueues::<u32, 8>::new();
    for (tag, tid) in [(1, BE), (2, VO), (3, BE), (4, VO)] {
        assert!(queues.push(tid, msdu(tag, tid)).is_ok());
    }
    assert_eq!(queues.pop(BE).map(|m| m.memory), Some(1));
    assert_eq!(queues.pop(BE).map(|m| m.memory), Some(3));
    assert_eq!(queues.pop(BE).map(|m| m.memory), None);
    assert_eq!(queues.pop(VO).map(|m| m.memory), Some(2));
    assert!(queues.push_front(VO, msdu(0, VO)).is_ok());
    assert_eq!(
        queues.pop(VO).map(|m| m.memory),
        Some(0),
        "put back at its head"
    );
    assert_eq!(queues.pop(VO).map(|m| m.memory), Some(4));
    assert!(queues.is_empty());
}

/// A TID blocked by its window holds only its own queue: the other TID of
/// the same access category is chosen while it waits, and a settlement
/// makes it ready again.
#[test]
fn a_tid_blocked_by_its_window_does_not_hold_another_tid_of_its_category() {
    let mut queues = TxQueues::<u32, 8>::new();
    assert!(queues.push(BE, msdu(1, BE)).is_ok());
    assert!(queues.push(BE3, msdu(2, BE3)).is_ok());
    queues.block(BE);
    assert_eq!(queues.select(None), Some(BE3));
    assert!(!queues.ready(BE));
    queues.block(BE3);
    assert_eq!(queues.select(None), None, "both wait for their windows");
    assert!(!queues.has_ready());
    queues.unblock();
    assert!(queues.select(None).is_some());
}

/// Voice goes before a best-effort backlog, and a held TID is passed over.
#[test]
fn voice_goes_before_a_best_effort_backlog_and_a_held_tid_is_passed_over() {
    let mut queues = TxQueues::<u32, 16>::new();
    for tag in 0..8 {
        assert!(queues.push(BE, msdu(tag, BE)).is_ok());
    }
    assert_eq!(queues.select(None), Some(BE));
    assert!(queues.push(VI, msdu(100, VI)).is_ok());
    assert_eq!(queues.select(None), Some(VI));
    assert!(queues.push(VO, msdu(101, VO)).is_ok());
    assert_eq!(
        queues.select(None),
        Some(VO),
        "the new voice MSDU waits for no backlog"
    );
    assert_eq!(queues.select(Some(VO)), Some(VI), "the held TID waits");
}

/// Within one access category the queue that had the least air goes next,
/// each attempt charged once; one of unknown cost is charged the dearest
/// known, never nothing; a queue that starts holding MSDUs takes no credit
/// for its idle time.
#[test]
fn within_a_category_the_least_served_queue_goes_next() {
    let mut queues = TxQueues::<u32, 16>::new();
    for tag in 0..4 {
        assert!(queues.push(BE, msdu(tag, BE)).is_ok());
        assert!(queues.push(BE3, msdu(10 + tag, BE3)).is_ok());
    }
    let mut served = Vec::new();
    for _ in 0..4 {
        let tid = queues.select(None).unwrap();
        served.push(tid);
        let _msdu = queues.pop(tid);
        queues.charge(tid, estimated(1000));
    }
    assert_eq!(
        served.iter().filter(|tid| **tid == BE).count(),
        2,
        "{served:?}"
    );
    // An unknown cost takes the queue's dearest known.
    queues.charge(BE, estimated(3000));
    queues.charge(BE, TxEstimate::Unknown);
    assert_eq!(queues.select(None), Some(BE3), "BE took 3000 + 3000 more");
    // A queue that empties and comes back starts level, with no banked air.
    while queues.pop(BE3).is_some() {}
    queues.charge(BE, estimated(50_000));
    while queues.pop(BE).is_some() {}
    assert!(queues.push(BE, msdu(1, BE)).is_ok());
    assert!(queues.push(BE3, msdu(2, BE3)).is_ok());
    let first = queues.select(None).unwrap();
    let _msdu = queues.pop(first);
    queues.charge(first, estimated(1000));
    assert_ne!(queues.select(None), Some(first), "the other goes next");
}

/// A full pool refuses with the MSDU back; a stop takes every MSDU, once.
#[test]
fn a_full_pool_refuses_and_a_drain_returns_every_msdu_once() {
    let mut queues = TxQueues::<u32, 4>::new();
    for tag in 0..4 {
        assert!(
            queues
                .push(WmmUserPriority::new(tag as u8 % 8).unwrap(), msdu(tag, BE))
                .is_ok()
        );
    }
    assert!(!queues.has_room());
    assert_eq!(
        queues.push(BE, msdu(9, BE)).err().map(|m| m.memory),
        Some(9)
    );
    let mut back = Vec::new();
    queues.drain(|msdu| back.push(msdu.memory));
    back.sort_unstable();
    assert_eq!(back, [0, 1, 2, 3]);
    assert!(queues.is_empty() && queues.has_room());
    // Every place is free again.
    for tag in 0..4 {
        assert!(queues.push(VO, msdu(tag, VO)).is_ok());
    }
    let mut any = Vec::new();
    while let Some(msdu) = queues.pop_any() {
        any.push(msdu.memory);
    }
    assert_eq!(any, [0, 1, 2, 3]);
}
