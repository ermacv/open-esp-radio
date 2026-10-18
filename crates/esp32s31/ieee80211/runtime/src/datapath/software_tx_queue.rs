//! Role-neutral indexed software TX queues.
//!
//! Payload owners live in one bounded arena. Queueing policy stores only
//! indices and a small flow key, so adding a peer/TID queue never duplicates
//! packet storage or reserves a private DMA-sized ring. Roles remain
//! responsible for constructing and validating their keys.

#[derive(Clone, Copy)]
struct IndexedFlow<K> {
    key: K,
    head: u8,
    tail: u8,
    len: u8,
}

/// Single owner of retained packet leases referenced by indexed queues.
pub(crate) struct IndexedLeaseArena<B, const FRAME_CAPACITY: usize> {
    frames: [Option<B>; FRAME_CAPACITY],
    free: [u8; FRAME_CAPACITY],
    free_len: u8,
    initialized: bool,
}

impl<B, const FRAME_CAPACITY: usize> IndexedLeaseArena<B, FRAME_CAPACITY> {
    pub(crate) const fn new() -> Self {
        assert!(FRAME_CAPACITY > 0, "an indexed lease arena cannot be empty");
        assert!(
            FRAME_CAPACITY <= u8::MAX as usize,
            "an indexed lease arena uses byte-sized indices"
        );
        Self {
            frames: [const { None }; FRAME_CAPACITY],
            free: [0; FRAME_CAPACITY],
            free_len: 0,
            initialized: false,
        }
    }

    fn initialize_free_list(&mut self) {
        if self.initialized {
            return;
        }
        for (index, free) in self.free.iter_mut().enumerate() {
            *free = u8::try_from(index).expect("validated lease index fits u8");
        }
        self.free_len =
            u8::try_from(FRAME_CAPACITY).expect("validated lease arena capacity fits u8");
        self.initialized = true;
    }

    pub(crate) fn insert(&mut self, frame: B) -> Result<u8, B> {
        self.initialize_free_list();
        if self.free_len == 0 {
            return Err(frame);
        }
        self.free_len -= 1;
        let index = self.free[usize::from(self.free_len)];
        self.frames[usize::from(index)] = Some(frame);
        Ok(index)
    }

    pub(crate) const fn remaining_capacity(&self) -> usize {
        if self.initialized {
            self.free_len as usize
        } else {
            FRAME_CAPACITY
        }
    }

    pub(crate) fn take(&mut self, index: u8) -> B {
        let frame = self.frames[usize::from(index)]
            .take()
            .expect("queue index names one retained lease");
        self.free[usize::from(self.free_len)] = index;
        self.free_len += 1;
        frame
    }

    pub(crate) fn get(&self, index: u8) -> &B {
        self.frames[usize::from(index)]
            .as_ref()
            .expect("queue or release index names one retained lease")
    }

    /// Release all software owners at an epoch boundary without moving the
    /// backing array through the caller's stack. No queue indices may survive.
    pub(crate) fn clear(&mut self) {
        for frame in &mut self.frames {
            drop(frame.take());
        }
        self.initialized = false;
        self.free_len = 0;
    }
}

/// Intrusive per-flow FIFOs with round-robin selection over active flows.
///
/// `FLOW_CAPACITY` bounds metadata, while `FRAME_CAPACITY` bounds the shared
/// owner arena. No payload capacity is reserved for an individual flow.
pub(crate) struct RoundRobinTxQueues<K, const FLOW_CAPACITY: usize, const FRAME_CAPACITY: usize> {
    next: [Option<u8>; FRAME_CAPACITY],
    flows: [Option<IndexedFlow<K>>; FLOW_CAPACITY],
    scheduler_cursor: u8,
    len: u8,
}

impl<K, const FLOW_CAPACITY: usize, const FRAME_CAPACITY: usize>
    RoundRobinTxQueues<K, FLOW_CAPACITY, FRAME_CAPACITY>
where
    K: Copy + Eq,
{
    pub(crate) const fn new() -> Self {
        assert!(FLOW_CAPACITY > 0, "a TXQ set must contain a flow slot");
        assert!(
            FLOW_CAPACITY <= u8::MAX as usize,
            "indexed TX queues use byte-sized flow indices"
        );
        assert!(
            FRAME_CAPACITY <= u8::MAX as usize,
            "indexed TX queues use byte-sized frame indices"
        );
        Self {
            next: [None; FRAME_CAPACITY],
            flows: [const { None }; FLOW_CAPACITY],
            scheduler_cursor: 0,
            len: 0,
        }
    }

    fn flow_index(&self, key: K) -> Option<usize> {
        self.flows
            .iter()
            .position(|flow| flow.is_some_and(|flow| flow.key == key))
    }

    pub(crate) fn push_back(&mut self, key: K, frame_index: u8) -> Result<(), u8> {
        let flow_index = if let Some(index) = self.flow_index(key) {
            index
        } else if let Some(index) = self.flows.iter().position(Option::is_none) {
            index
        } else {
            return Err(frame_index);
        };
        debug_assert!(usize::from(frame_index) < FRAME_CAPACITY);
        debug_assert!(self.next[usize::from(frame_index)].is_none());
        if let Some(flow) = self.flows[flow_index].as_mut() {
            self.next[usize::from(flow.tail)] = Some(frame_index);
            flow.tail = frame_index;
            flow.len = flow
                .len
                .checked_add(1)
                .expect("one flow cannot exceed the bounded frame arena");
        } else {
            self.flows[flow_index] = Some(IndexedFlow {
                key,
                head: frame_index,
                tail: frame_index,
                len: 1,
            });
        }
        self.len = self
            .len
            .checked_add(1)
            .expect("TXQ length is bounded by its frame arena");
        Ok(())
    }

    /// Restore an older frame ahead of every still-queued frame for `key`.
    ///
    /// A caller uses this only when a frame was removed speculatively and a
    /// lower-layer admission step failed. Appending that frame would let
    /// younger packets overtake it and corrupt the per-flow FIFO contract.
    pub(crate) fn push_front(&mut self, key: K, frame_index: u8) -> Result<(), u8> {
        let flow_index = if let Some(index) = self.flow_index(key) {
            index
        } else if let Some(index) = self.flows.iter().position(Option::is_none) {
            index
        } else {
            return Err(frame_index);
        };
        debug_assert!(usize::from(frame_index) < FRAME_CAPACITY);
        debug_assert!(self.next[usize::from(frame_index)].is_none());
        if let Some(flow) = self.flows[flow_index].as_mut() {
            self.next[usize::from(frame_index)] = Some(flow.head);
            flow.head = frame_index;
            flow.len = flow
                .len
                .checked_add(1)
                .expect("one flow cannot exceed the bounded frame arena");
        } else {
            self.flows[flow_index] = Some(IndexedFlow {
                key,
                head: frame_index,
                tail: frame_index,
                len: 1,
            });
        }
        self.len = self
            .len
            .checked_add(1)
            .expect("TXQ length is bounded by its frame arena");
        Ok(())
    }

    fn pop_flow_index(&mut self, flow_index: usize) -> Option<u8> {
        let mut flow = self.flows.get(flow_index).copied().flatten()?;
        let index = flow.head;
        let next = self.next[usize::from(index)].take();
        flow.len -= 1;
        if flow.len == 0 {
            self.flows[flow_index] = None;
        } else {
            flow.head = next.expect("a nonterminal TXQ frame has a successor");
            self.flows[flow_index] = Some(flow);
        }
        self.len -= 1;
        Some(index)
    }

    pub(crate) fn pop_key(&mut self, key: K) -> Option<u8> {
        let flow = self.flow_index(key)?;
        self.pop_flow_index(flow)
    }

    /// Inspect retained heads without advancing the scheduler or taking slots.
    pub(crate) fn heads(&self) -> impl Iterator<Item = (K, u8)> + '_ {
        self.flows
            .iter()
            .flatten()
            .map(|flow| (flow.key, flow.head))
    }

    fn next_active_flow(&self) -> Option<usize> {
        for offset in 0..FLOW_CAPACITY {
            let index = (usize::from(self.scheduler_cursor) + offset) % FLOW_CAPACITY;
            if self.flows[index].is_some() {
                return Some(index);
            }
        }
        None
    }

    pub(crate) fn pop_scheduled(&mut self) -> Option<(K, u8)> {
        let index = self.next_active_flow()?;
        let key = self.flows[index]
            .expect("the scheduler scan names an occupied flow")
            .key;
        self.scheduler_cursor =
            u8::try_from((index + 1) % FLOW_CAPACITY).expect("flow index fits u8");
        self.pop_flow_index(index).map(|frame| (key, frame))
    }

    pub(crate) fn len_for(&self, key: K) -> usize {
        self.flow_index(key)
            .and_then(|index| self.flows[index])
            .map_or(0, |flow| usize::from(flow.len))
    }

    pub(crate) fn scheduled_len(&self) -> usize {
        self.next_active_flow()
            .and_then(|index| self.flows[index])
            .map_or(0, |flow| usize::from(flow.len))
    }

    pub(crate) const fn len(&self) -> usize {
        self.len as usize
    }
}

#[cfg(test)]
mod tests;
