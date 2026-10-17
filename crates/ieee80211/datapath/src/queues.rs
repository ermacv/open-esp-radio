//! Hierarchical queues sharing one bounded owner arena.
//!
//! The caller selects an outer key; each take serves the next inner flow.
//! Flow count cannot multiply outer scheduling turns. Synchronization,
//! classification and radio eligibility belong to the caller.

struct Entry<T> {
    value: Option<T>,
    next: Option<usize>,
}

#[derive(Clone, Copy)]
struct Queue<K> {
    key: K,
    head: usize,
    tail: usize,
    len: usize,
}

struct Flow<F> {
    key: F,
    queue: usize,
    head: usize,
    tail: usize,
    next: Option<usize>,
}

/// One owner arena, with outer queues of round-robin inner FIFOs.
///
/// A free owner slot always permits publication, including a new outer key
/// or flow. Neither level reserves payload capacity. Default `F = ()` gives
/// one FIFO per key; explicit flow keys enable inner packet round-robin.
/// This policy does not account for bytes, airtime or admission fairness.
pub struct TxQueues<K, T, const CAPACITY: usize, F = ()> {
    entries: [Entry<T>; CAPACITY],
    queues: [Option<Queue<K>>; CAPACITY],
    flows: [Option<Flow<F>>; CAPACITY],
    free: Option<usize>,
    len: usize,
}

impl<K: Copy + Eq, T, const CAPACITY: usize, F: Copy + Eq> TxQueues<K, T, CAPACITY, F> {
    pub const fn new() -> Self {
        assert!(CAPACITY > 0, "TX queues need at least one owner slot");
        let mut entries = [const {
            Entry {
                value: None,
                next: None,
            }
        }; CAPACITY];
        let mut index = 0;
        while index + 1 < CAPACITY {
            entries[index].next = Some(index + 1);
            index += 1;
        }
        Self {
            entries,
            queues: [const { None }; CAPACITY],
            flows: [const { None }; CAPACITY],
            free: Some(0),
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub const fn is_full(&self) -> bool {
        self.len == CAPACITY
    }

    fn queue_index(&self, key: K) -> Option<usize> {
        self.queues
            .iter()
            .position(|q| q.as_ref().is_some_and(|q| q.key == key))
    }

    /// Total frames across every flow of the selected outer key.
    pub fn len_for(&self, key: K) -> usize {
        self.queue_index(key)
            .map_or(0, |i| self.queues[i].as_ref().expect("occupied queue").len)
    }

    /// Append to one flow, returning the unchanged owner on exhaustion.
    /// Only metadata is inspected; queued packet bytes are never revisited.
    pub fn push_flow(&mut self, key: K, flow_key: F, value: T) -> Result<(), T> {
        let Some(index) = self.free else {
            return Err(value);
        };
        let queue_index = self.queue_index(key).unwrap_or_else(|| {
            self.queues
                .iter()
                .position(Option::is_none)
                .expect("a free owner slot guarantees outer metadata")
        });
        let existing = self.flows.iter().position(|f| {
            f.as_ref()
                .is_some_and(|f| f.queue == queue_index && f.key == flow_key)
        });
        let flow_index = existing.unwrap_or_else(|| {
            self.flows
                .iter()
                .position(Option::is_none)
                .expect("a free owner slot guarantees flow metadata")
        });
        self.free = self.entries[index].next.take();
        self.entries[index].value = Some(value);
        if let Some(flow) = self.flows[flow_index].as_mut() {
            self.entries[flow.tail].next = Some(index);
            flow.tail = index;
        } else {
            self.flows[flow_index] = Some(Flow {
                key: flow_key,
                queue: queue_index,
                head: index,
                tail: index,
                next: None,
            });
            if let Some(queue) = self.queues[queue_index].as_mut() {
                self.flows[queue.tail]
                    .as_mut()
                    .expect("outer tail owns a flow")
                    .next = Some(flow_index);
                queue.tail = flow_index;
            } else {
                self.queues[queue_index] = Some(Queue {
                    key,
                    head: flow_index,
                    tail: flow_index,
                    len: 0,
                });
            }
        }
        self.queues[queue_index]
            .as_mut()
            .expect("published outer queue")
            .len += 1;
        self.len += 1;
        Ok(())
    }

    /// Serve one frame of the next flow, preserving FIFO within that flow.
    /// Other outer keys are untouched. Nonempty flows rotate to the tail;
    /// empty flow metadata is immediately reusable by any outer key.
    pub fn pop(&mut self, key: K) -> Option<T> {
        let queue_index = self.queue_index(key)?;
        let queue = self.queues[queue_index].as_mut().expect("occupied queue");
        let flow_index = queue.head;
        let mut flow = self.flows[flow_index]
            .take()
            .expect("outer head owns a flow");
        let index = flow.head;
        queue.len -= 1;
        if let Some(next) = self.entries[index].next {
            flow.head = next;
            if let Some(next_flow) = flow.next.take() {
                queue.head = next_flow;
                self.flows[queue.tail]
                    .as_mut()
                    .expect("outer tail owns a flow")
                    .next = Some(flow_index);
                queue.tail = flow_index;
            }
            self.flows[flow_index] = Some(flow);
        } else if let Some(next_flow) = flow.next {
            queue.head = next_flow;
        } else {
            self.queues[queue_index] = None;
        }
        let value = self.entries[index]
            .value
            .take()
            .expect("flow owns its entry");
        self.entries[index].next = self.free;
        self.free = Some(index);
        self.len -= 1;
        Some(value)
    }

    /// Inspect the next packet of each outer queue without rotating a flow or
    /// taking an owner. Radio admission can estimate a minimum useful grant
    /// before reserving airtime or DMA. Each outer key appears exactly once,
    /// regardless of its number of transport flows.
    pub fn heads(&self) -> impl Iterator<Item = (K, &T)> {
        self.queues
            .iter()
            .flatten()
            .map(|queue| (queue.key, self.head_packet(queue)))
    }

    /// Borrow the next selected inner-flow packet and total outer backlog.
    /// This does not rotate either scheduling level or transfer ownership.
    pub fn head(&self, key: K) -> Option<(&T, usize)> {
        let queue = self.queues[self.queue_index(key)?].as_ref()?;
        Some((self.head_packet(queue), queue.len))
    }

    fn head_packet(&self, queue: &Queue<K>) -> &T {
        let flow = self.flows[queue.head]
            .as_ref()
            .expect("queue owns its head flow");
        self.entries[flow.head]
            .value
            .as_ref()
            .expect("flow owns its head packet")
    }

    /// First nonempty key strictly after `after`, wrapping to the lowest key.
    /// The order is independent of metadata-slot reuse and inner flow count.
    /// Inspection does not advance a cursor or claim an owner.
    pub fn next_head_after(&self, after: Option<K>) -> Option<(K, &T, usize)>
    where
        K: Ord,
    {
        let queue = self
            .queues
            .iter()
            .flatten()
            .min_by_key(|queue| (after.is_some_and(|after| queue.key <= after), queue.key))?;
        Some((queue.key, self.head_packet(queue), queue.len))
    }

    /// Enumerate outer keys, once per key regardless of its inner flow count.
    /// Only the caller decides when to advance to another outer queue.
    pub fn next_key(&self, cursor: &mut usize) -> Option<K> {
        for offset in 0..CAPACITY {
            let index = (*cursor % CAPACITY + offset) % CAPACITY;
            if let Some(queue) = self.queues[index].as_ref() {
                *cursor = (index + 1) % CAPACITY;
                return Some(queue.key);
            }
        }
        None
    }
}

impl<K: Copy + Eq, T, const CAPACITY: usize> TxQueues<K, T, CAPACITY> {
    pub fn push(&mut self, key: K, value: T) -> Result<(), T> {
        self.push_flow(key, (), value)
    }
}

impl<K: Copy + Eq, T, const CAPACITY: usize, F: Copy + Eq> Default for TxQueues<K, T, CAPACITY, F> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
