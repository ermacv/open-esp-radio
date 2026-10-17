use super::TxQueues;

mod hierarchy;

extern crate std;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[test]
fn selected_queue_preserves_order_without_consuming_other_owners() {
    let mut queues = TxQueues::<_, _, 8>::new();
    for sequence in 0..4 {
        queues.push('A', sequence).unwrap();
        queues.push('B', sequence).unwrap();
    }
    for sequence in 0..4 {
        assert_eq!(queues.pop('B'), Some(sequence));
        assert_eq!(queues.len_for('A'), 4);
    }
    assert_eq!(queues.pop('C'), None);
    for sequence in 0..4 {
        assert_eq!(queues.pop('A'), Some(sequence));
    }
    assert!(queues.is_empty());
}

#[test]
fn free_owner_capacity_always_admits_a_new_destination() {
    let mut queues = TxQueues::<_, _, 4>::new();
    for key in 0..4 {
        queues.push(key, key).unwrap();
    }
    assert_eq!(queues.push(5, 50), Err(50));
    assert_eq!(queues.pop(2), Some(2));
    queues.push(5, 50).unwrap();
    assert_eq!(queues.pop(5), Some(50));
    assert_eq!(queues.len(), 3);
}

#[test]
fn cyclic_selection_does_not_charge_aggregate_members_as_new_turns() {
    let mut queues = TxQueues::<_, _, 5>::new();
    for n in 0..3 {
        queues.push('A', n).unwrap();
    }
    queues.push('B', 10).unwrap();
    let mut cursor = 0;
    let first = queues.next_key(&mut cursor).unwrap();
    assert_eq!(first, 'A');
    assert_eq!(queues.pop(first), Some(0));
    assert_eq!(queues.pop(first), Some(1));
    assert_eq!(queues.next_key(&mut cursor), Some('B'));
    assert_eq!(queues.next_key(&mut cursor), Some('A'));
}

#[test]
fn mixed_publication_selection_and_slot_reuse_match_independent_fifos() {
    let mut queues = TxQueues::<_, _, 17>::new();
    let mut model: BTreeMap<u8, VecDeque<u32>> = BTreeMap::new();
    let mut random = 1_u32;
    for sequence in 0..10_000 {
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        let key = ((random >> 8) % 23) as u8;
        if random & 1 == 0 {
            let count: usize = model.values().map(VecDeque::len).sum();
            let result = queues.push(key, sequence);
            if count == 17 {
                assert_eq!(result, Err(sequence));
            } else {
                result.unwrap();
                model.entry(key).or_default().push_back(sequence);
            }
        } else {
            assert_eq!(
                queues.pop(key),
                model.get_mut(&key).and_then(VecDeque::pop_front)
            );
        }
        assert_eq!(
            queues.len(),
            model.values().map(VecDeque::len).sum::<usize>()
        );
        for (&key, expected) in &model {
            assert_eq!(queues.len_for(key), expected.len());
        }
    }
}

#[derive(Debug)]
struct Owner(Arc<AtomicUsize>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn exhaustion_and_queue_destruction_return_every_owner_exactly_once() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut queues = TxQueues::<_, _, 2>::new();
    queues.push('A', Owner(drops.clone())).unwrap();
    queues.push('B', Owner(drops.clone())).unwrap();
    let rejected = queues.push('C', Owner(drops.clone())).unwrap_err();
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    drop(rejected);
    let selected = queues.pop('B').unwrap();
    drop(queues);
    assert_eq!(drops.load(Ordering::Relaxed), 2);
    drop(selected);
    assert_eq!(drops.load(Ordering::Relaxed), 3);
}
