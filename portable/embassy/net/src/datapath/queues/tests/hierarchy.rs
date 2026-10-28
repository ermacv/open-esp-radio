use super::*;

#[test]
fn sparse_flow_is_served_without_draining_the_bulk_flow() {
    let mut queues = TxQueues::<_, _, 16, _>::new();
    for sequence in 0..14 {
        queues.push_flow('A', 1, sequence).unwrap();
    }
    queues.push_flow('A', 2, 99).unwrap();
    queues.push_flow('B', 1, 100).unwrap();
    assert_eq!(queues.pop('A'), Some(0));
    assert_eq!(queues.pop('A'), Some(99));
    assert_eq!(queues.len_for('B'), 1);
    for sequence in 1..14 {
        assert_eq!(queues.pop('A'), Some(sequence));
    }
    assert_eq!(queues.pop('B'), Some(100));
}

#[test]
fn more_transport_flows_do_not_create_more_outer_turns() {
    let mut queues = TxQueues::<_, _, 16, _>::new();
    for flow in 0..10 {
        queues.push_flow('A', flow, flow).unwrap();
    }
    queues.push_flow('B', 0, 99).unwrap();
    let mut cursor = 0;
    for _ in 0..3 {
        assert_eq!(queues.next_key(&mut cursor), Some('A'));
        assert_eq!(queues.next_key(&mut cursor), Some('B'));
    }
    assert_eq!(queues.len_for('A'), 10);
}

#[test]
fn flow_turn_survives_refill_and_new_flow_admission() {
    let mut queues = TxQueues::<_, _, 8, _>::new();
    for (flow, value) in [(1, 10), (1, 11), (2, 20), (2, 21)] {
        queues.push_flow('A', flow, value).unwrap();
    }
    assert_eq!(queues.pop('A'), Some(10));
    queues.push_flow('A', 1, 12).unwrap();
    queues.push_flow('A', 3, 30).unwrap();
    for value in [20, 11, 30, 21, 12] {
        assert_eq!(queues.pop('A'), Some(value));
    }
    assert!(queues.is_empty());
}

#[test]
fn each_free_owner_can_admit_a_new_flow_or_destination() {
    let mut queues = TxQueues::<_, _, 3, _>::new();
    queues.push_flow('A', 1, 10).unwrap();
    queues.push_flow('A', 2, 20).unwrap();
    queues.push_flow('B', 1, 30).unwrap();
    assert_eq!(queues.push_flow('C', 9, 40), Err(40));
    assert_eq!(queues.pop('A'), Some(10));
    queues.push_flow('C', 9, 40).unwrap();
    assert_eq!(queues.pop('A'), Some(20));
    assert_eq!(queues.pop('B'), Some(30));
    assert_eq!(queues.pop('C'), Some(40));
}

#[test]
fn mixed_reuse_matches_independent_nested_fifos_and_round_robin() {
    type Flows = VecDeque<(u8, VecDeque<u32>)>;
    let mut model: BTreeMap<u8, Flows> = BTreeMap::new();
    let mut queues = TxQueues::<_, _, 17, _>::new();
    let mut random = 1_u32;
    for sequence in 0..10_000 {
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        let destination = ((random >> 8) % 7) as u8;
        let flow = ((random >> 16) % 13) as u8;
        if random & 1 == 0 {
            let count: usize = model
                .values()
                .flat_map(|flows| flows.iter())
                .map(|(_, fifo)| fifo.len())
                .sum();
            let result = queues.push_flow(destination, flow, sequence);
            if count == 17 {
                assert_eq!(result, Err(sequence));
            } else {
                result.unwrap();
                let flows = model.entry(destination).or_default();
                if let Some((_, fifo)) = flows.iter_mut().find(|(key, _)| *key == flow) {
                    fifo.push_back(sequence);
                } else {
                    flows.push_back((flow, VecDeque::from([sequence])));
                }
            }
        } else {
            let expected = model.get_mut(&destination).and_then(|flows| {
                let (flow, mut fifo) = flows.pop_front()?;
                let value = fifo.pop_front();
                if !fifo.is_empty() {
                    flows.push_back((flow, fifo));
                }
                value
            });
            assert_eq!(queues.pop(destination), expected);
        }
        for (&key, flows) in &model {
            assert_eq!(
                queues.len_for(key),
                flows.iter().map(|(_, fifo)| fifo.len()).sum()
            );
        }
    }
}
