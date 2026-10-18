use super::{IndexedLeaseArena, RoundRobinTxQueues};

const FLOW_A: u8 = 1;
const FLOW_B: u8 = 2;
const FLOW_CAPACITY: usize = 4;
const FRAME_CAPACITY: usize = 8;

struct TestQueues<B> {
    leases: IndexedLeaseArena<B, FRAME_CAPACITY>,
    queues: RoundRobinTxQueues<u8, FLOW_CAPACITY, FRAME_CAPACITY>,
}

impl<B> TestQueues<B> {
    const fn new() -> Self {
        Self {
            leases: IndexedLeaseArena::new(),
            queues: RoundRobinTxQueues::new(),
        }
    }

    fn push(&mut self, key: u8, frame: B) -> Result<(), B> {
        let index = self.leases.insert(frame)?;
        if self.queues.push_back(key, index).is_err() {
            return Err(self.leases.take(index));
        }
        Ok(())
    }

    fn push_front(&mut self, key: u8, frame: B) -> Result<(), B> {
        let index = self.leases.insert(frame)?;
        if self.queues.push_front(key, index).is_err() {
            return Err(self.leases.take(index));
        }
        Ok(())
    }

    fn pop_key(&mut self, key: u8) -> Option<B> {
        self.queues
            .pop_key(key)
            .map(|index| self.leases.take(index))
    }

    fn pop_scheduled(&mut self) -> Option<(u8, B)> {
        self.queues
            .pop_scheduled()
            .map(|(key, index)| (key, self.leases.take(index)))
    }
}

#[test]
fn interleaved_frames_keep_per_flow_order() {
    let mut queues = TestQueues::new();
    for sequence in 0..4_u8 {
        queues.push(FLOW_A, sequence * 2).unwrap();
        queues.push(FLOW_B, sequence * 2 + 1).unwrap();
    }

    assert_eq!(queues.queues.len_for(FLOW_A), 4);
    assert_eq!(queues.queues.len_for(FLOW_B), 4);
    for sequence in 0..4_u8 {
        assert_eq!(queues.pop_key(FLOW_A), Some(sequence * 2));
    }
    for sequence in 0..4_u8 {
        assert_eq!(queues.pop_key(FLOW_B), Some(sequence * 2 + 1));
    }
    assert_eq!(queues.queues.len(), 0);
}

#[test]
fn scheduler_round_robins_nonempty_flows() {
    let mut queues = TestQueues::new();
    queues.push(FLOW_A, 10).unwrap();
    queues.push(FLOW_A, 11).unwrap();
    queues.push(FLOW_B, 20).unwrap();
    queues.push(FLOW_B, 21).unwrap();

    assert_eq!(queues.pop_scheduled(), Some((FLOW_A, 10)));
    assert_eq!(queues.pop_scheduled(), Some((FLOW_B, 20)));
    assert_eq!(queues.pop_scheduled(), Some((FLOW_A, 11)));
    assert_eq!(queues.pop_scheduled(), Some((FLOW_B, 21)));
    assert_eq!(queues.queues.scheduled_len(), 0);
}

#[test]
fn arena_reuses_every_bounded_frame_slot() {
    let mut queues = TestQueues::new();
    for value in 0..FRAME_CAPACITY {
        queues.push(FLOW_A, value).unwrap();
    }
    assert_eq!(queues.push(FLOW_A, usize::MAX), Err(usize::MAX));
    for value in 0..FRAME_CAPACITY {
        assert_eq!(queues.pop_key(FLOW_A), Some(value));
    }
    queues.push(FLOW_B, 42).unwrap();
    assert_eq!(queues.pop_key(FLOW_B), Some(42));
}

#[test]
fn failed_admission_restores_the_original_flow_prefix() {
    let mut queues = TestQueues::new();
    queues.push(FLOW_A, 12).unwrap();
    queues.push(FLOW_A, 13).unwrap();

    // Frames 10 and 11 were speculatively removed before 12 and 13.
    // Restore in reverse insertion order so their original prefix is
    // visible to the next scheduler turn.
    queues.push_front(FLOW_A, 11).unwrap();
    queues.push_front(FLOW_A, 10).unwrap();

    for expected in 10..14 {
        assert_eq!(queues.pop_key(FLOW_A), Some(expected));
    }
}

#[test]
fn full_flow_table_returns_the_unqueued_index() {
    let mut queues = TestQueues::new();
    for flow in 0..FLOW_CAPACITY as u8 {
        queues.push(flow, flow).unwrap();
    }
    assert_eq!(queues.push(99, 99), Err(99));
}
