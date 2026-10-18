use super::{
    ApActiveFrameQueues, ApAssociationIdentity, ApFrameLeaseArena, ApPowerSaveFrameQueue,
    ApTxFlowKey, aggregate_adapter_available,
};

#[cfg(feature = "owned-network")]
mod batch_demand;
#[cfg(feature = "owned-network")]
mod fifo;
#[cfg(feature = "owned-network")]
mod power_save_admission;
#[cfg(feature = "owned-network")]
mod selection;
#[cfg(feature = "owned-network")]
mod support;

mod retention;

#[cfg(feature = "owned-network")]
impl<N> super::ApTxSelection<N> {
    fn expect_frame(self) -> (ApTxFlowKey, N) {
        match self {
            Self::Frame(key, frame) => (key, frame),
            Self::Buffered(_) => panic!("expected an unbuffered selected frame"),
        }
    }
}

struct TestActiveArena<B> {
    leases: ApFrameLeaseArena<B>,
    queues: ApActiveFrameQueues,
}

impl<B> TestActiveArena<B> {
    const fn new() -> Self {
        Self {
            leases: ApFrameLeaseArena::new(),
            queues: ApActiveFrameQueues::new(),
        }
    }

    fn push(&mut self, key: ApTxFlowKey, frame: B) -> Result<(), B> {
        let index = self.leases.insert(frame)?;
        if self.queues.push_back(key, index).is_err() {
            return Err(self.leases.take(index));
        }
        Ok(())
    }

    fn pop_key(&mut self, key: ApTxFlowKey) -> Option<B> {
        self.queues
            .pop_key(key)
            .map(|index| self.leases.take(index))
    }

    fn pop_scheduled(&mut self) -> Option<(ApTxFlowKey, B)> {
        self.queues
            .pop_scheduled()
            .map(|(key, index)| (key, self.leases.take(index)))
    }

    fn len_for(&self, key: ApTxFlowKey) -> usize {
        self.queues.len_for(key)
    }

    fn len(&self) -> usize {
        self.queues.len()
    }
}

const FLOW_A: ApTxFlowKey =
    ApTxFlowKey::associated(ApAssociationIdentity::new([0x02, 0, 0, 0, 0, 1], 1, 10).unwrap());
const FLOW_B: ApTxFlowKey =
    ApTxFlowKey::associated(ApAssociationIdentity::new([0x02, 0, 0, 0, 0, 2], 2, 11).unwrap());
const FLOW_A_REASSOCIATED: ApTxFlowKey =
    ApTxFlowKey::associated(ApAssociationIdentity::new([0x02, 0, 0, 0, 0, 1], 1, 12).unwrap());

#[test]
fn ordinary_publication_blocks_aggregate_adapter_borrow() {
    assert!(aggregate_adapter_available(false));
    assert!(!aggregate_adapter_available(true));
}

#[test]
fn generation_bound_flow_key_stays_compact() {
    assert_eq!(core::mem::size_of::<ApTxFlowKey>(), 12);
}

#[test]
fn active_arena_regroups_interleaved_frames_without_reordering_a_flow() {
    let mut arena = TestActiveArena::new();
    for sequence in 0..16_u8 {
        arena.push(FLOW_A, sequence * 2).unwrap();
        arena.push(FLOW_B, sequence * 2 + 1).unwrap();
    }

    assert_eq!(arena.len_for(FLOW_A), 16);
    assert_eq!(arena.len_for(FLOW_B), 16);
    for sequence in 0..16_u8 {
        assert_eq!(arena.pop_key(FLOW_A), Some(sequence * 2));
    }
    for sequence in 0..16_u8 {
        assert_eq!(arena.pop_key(FLOW_B), Some(sequence * 2 + 1));
    }
    assert_eq!(arena.len(), 0);
}

#[test]
fn active_arena_schedules_nonempty_flows_round_robin() {
    let mut arena = TestActiveArena::new();
    arena.push(FLOW_A, 10).unwrap();
    arena.push(FLOW_A, 11).unwrap();
    arena.push(FLOW_B, 20).unwrap();
    arena.push(FLOW_B, 21).unwrap();

    assert_eq!(arena.pop_scheduled(), Some((FLOW_A, 10)));
    assert_eq!(arena.pop_scheduled(), Some((FLOW_B, 20)));
    assert_eq!(arena.pop_scheduled(), Some((FLOW_A, 11)));
    assert_eq!(arena.pop_scheduled(), Some((FLOW_B, 21)));
}

#[test]
fn active_arena_never_merges_reused_peer_generations() {
    let mut arena = TestActiveArena::new();
    arena.push(FLOW_A, 10).unwrap();
    arena.push(FLOW_A_REASSOCIATED, 20).unwrap();

    assert_eq!(arena.len_for(FLOW_A), 1);
    assert_eq!(arena.len_for(FLOW_A_REASSOCIATED), 1);
    assert_eq!(arena.pop_key(FLOW_A), Some(10));
    assert_eq!(arena.pop_key(FLOW_A_REASSOCIATED), Some(20));
}

#[test]
fn power_save_queue_drops_only_the_stale_association_generation() {
    let first = FLOW_A.association().unwrap();
    let replacement = FLOW_A_REASSOCIATED.association().unwrap();
    let mut leases = ApFrameLeaseArena::new();
    let mut queue = ApPowerSaveFrameQueue::new();
    queue.push(first, 10, &mut leases).unwrap();
    queue.push(replacement, 20, &mut leases).unwrap();

    queue.retain(&mut leases, |identity| identity == replacement);

    assert_eq!(queue.len, 1);
    assert_eq!(queue.oldest_index_for(first), None);
    let index = queue.oldest_index_for(replacement).unwrap();
    let release = queue.take_at(index).unwrap();
    assert_eq!(*release.frame(&leases), 20);
    release.complete(&mut leases);
}

#[test]
fn active_arena_reuses_every_bounded_frame_slot() {
    let mut arena = TestActiveArena::new();
    for value in 0..super::AP_ACTIVE_FRAME_CAPACITY {
        arena.push(FLOW_A, value).unwrap();
    }
    assert_eq!(arena.push(FLOW_A, usize::MAX), Err(usize::MAX));
    for value in 0..super::AP_ACTIVE_FRAME_CAPACITY {
        assert_eq!(arena.pop_key(FLOW_A), Some(value));
    }
    arena.push(FLOW_B, 42).unwrap();
    assert_eq!(arena.pop_key(FLOW_B), Some(42));
}

#[cfg(feature = "owned-network")]
mod airtime;
