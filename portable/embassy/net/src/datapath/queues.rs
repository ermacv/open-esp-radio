//! Hierarchical queues sharing one bounded owner arena.
//!
//! Each outer key holds a ring of inner flows; each flow is a FIFO of
//! entries linked by index in the arena, as the archive's indexed lease
//! arena and round-robin queues did. Publication appends to its flow's
//! tail (a new flow joins the ring's end); a take serves the head of the
//! ring's first flow and moves that flow to the ring's end while it holds
//! more. Neither moves an owner: no operation shifts slots or rescans the
//! arena per packet. Outer key slots keep their selection order.

/// One queued owner and the next entry of its flow, by arena index.
struct Entry<T> {
    value: T,
    next: Option<u8>,
}

/// One inner flow: its FIFO's ends and the next flow of its key's ring.
struct Flow<F> {
    flow: F,
    head: u8,
    tail: u8,
    next: Option<u8>,
}

/// One outer key: its ring of flows and its backlog.
struct Key<K> {
    key: K,
    first: u8,
    last: u8,
    len: usize,
}

/// One owner arena, with outer queues of round-robin inner FIFOs.
///
/// A free owner slot always permits publication, including a new outer key
/// or flow. Default `F = ()` gives one FIFO per key. Each take serves the
/// next inner flow of its key in turn.
/// This policy does not account for bytes, airtime or admission fairness.
pub struct TxQueues<K, T, const CAPACITY: usize, F = ()> {
    entries: [Option<Entry<T>>; CAPACITY],
    flows: [Option<Flow<F>>; CAPACITY],
    keys: [Option<Key<K>>; CAPACITY],
    len: usize,
}

impl<K: Copy + Eq, T, const CAPACITY: usize, F: Copy + Eq> TxQueues<K, T, CAPACITY, F> {
    /// An empty arena with capacity checked at compile time.
    ///
    /// ```compile_fail,E0080
    /// use oer_portable_embassy_net::datapath::TxQueues;
    /// let _ = TxQueues::<u8, u8, 0>::new();
    /// ```
    /// ```compile_fail,E0080
    /// use oer_portable_embassy_net::datapath::TxQueues;
    /// let _ = TxQueues::<u8, u8, 257>::new();
    /// ```
    pub const fn new() -> Self {
        const {
            if CAPACITY == 0 {
                panic!("TX queues need at least one owner slot");
            }
            if CAPACITY > u8::MAX as usize + 1 {
                panic!("TX queues index their owner slots by u8");
            }
        }
        Self {
            entries: [const { None }; CAPACITY],
            flows: [const { None }; CAPACITY],
            keys: [const { None }; CAPACITY],
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn is_full(&self) -> bool {
        self.len >= CAPACITY
    }

    /// Total frames across every flow of the selected outer key.
    pub fn len_for(&self, key: K) -> usize {
        self.key(key).map_or(0, |(_, slot)| slot.len)
    }

    /// The key's slot index and slot.
    fn key(&self, key: K) -> Option<(usize, &Key<K>)> {
        self.keys.iter().enumerate().find_map(|(index, slot)| {
            slot.as_ref()
                .filter(|slot| slot.key == key)
                .map(|slot| (index, slot))
        })
    }

    /// The first free slot of `slots`, as an arena index.
    fn free<S>(slots: &[Option<S>]) -> Option<u8> {
        slots
            .iter()
            .position(Option::is_none)
            .and_then(|index| u8::try_from(index).ok())
    }

    /// Append to one flow, returning the unchanged owner on exhaustion.
    pub fn push_flow(&mut self, key: K, flow: F, value: T) -> Result<(), T> {
        let Some(entry) = Self::free(&self.entries) else {
            return Err(value);
        };
        // A free entry proves fewer flows and keys than entries: a new
        // flow and a new key each find a free slot.
        let existing = self.key(key).map(|(index, slot)| (index, slot.first));
        let mut found = None;
        if let Some((_, first)) = existing {
            let mut at = Some(first);
            while let Some(index) = at {
                let Some(candidate) = self.flows.get(usize::from(index)).and_then(Option::as_ref)
                else {
                    break;
                };
                if candidate.flow == flow {
                    found = Some(index);
                    break;
                }
                at = candidate.next;
            }
        }
        let new_flow = if found.is_none() {
            let Some(index) = Self::free(&self.flows) else {
                return Err(value);
            };
            Some(index)
        } else {
            None
        };
        let new_key = if existing.is_none() {
            let Some(index) = self.keys.iter().position(Option::is_none) else {
                return Err(value);
            };
            Some(index)
        } else {
            None
        };
        let Some(entry_slot) = self.entries.get_mut(usize::from(entry)) else {
            return Err(value);
        };
        *entry_slot = Some(Entry { value, next: None });
        if let Some(index) = found {
            // Link behind the flow's tail.
            let tail = self
                .flows
                .get_mut(usize::from(index))
                .and_then(Option::as_mut)
                .map(|flow| core::mem::replace(&mut flow.tail, entry));
            if let Some(last) = tail
                .and_then(|tail| self.entries.get_mut(usize::from(tail)))
                .and_then(Option::as_mut)
            {
                last.next = Some(entry);
            }
        } else if let Some(index) = new_flow {
            if let Some(slot) = self.flows.get_mut(usize::from(index)) {
                *slot = Some(Flow {
                    flow,
                    head: entry,
                    tail: entry,
                    next: None,
                });
            }
            match (existing, new_key) {
                (Some((key_index, _)), _) => {
                    // The new flow joins the end of its key's ring.
                    let last = self
                        .keys
                        .get_mut(key_index)
                        .and_then(Option::as_mut)
                        .map(|slot| core::mem::replace(&mut slot.last, index));
                    if let Some(Some(Some(last))) =
                        last.map(|last| self.flows.get_mut(usize::from(last)).map(Option::as_mut))
                    {
                        last.next = Some(index);
                    }
                }
                (None, Some(key_index)) => {
                    if let Some(slot) = self.keys.get_mut(key_index) {
                        *slot = Some(Key {
                            key,
                            first: index,
                            last: index,
                            len: 0,
                        });
                    }
                }
                (None, None) => {}
            }
        }
        let key_index = existing.map(|(index, _)| index).or(new_key);
        if let Some(slot) = key_index
            .and_then(|index| self.keys.get_mut(index))
            .and_then(Option::as_mut)
        {
            // At most CAPACITY entries: the backlog fits.
            slot.len = slot.len.saturating_add(1);
        }
        self.len = self.len.saturating_add(1);
        Ok(())
    }

    /// Serve the oldest packet of the key's next flow, then move that flow
    /// to the end of the key's ring while it holds more.
    pub fn pop(&mut self, key: K) -> Option<T> {
        let (key_index, first) = self.key(key).map(|(index, slot)| (index, slot.first))?;
        let flow = self.flows.get_mut(usize::from(first))?.as_mut()?;
        let entry_index = flow.head;
        let entry = self.entries.get_mut(usize::from(entry_index))?.take()?;
        let rest = flow.next;
        let emptied = match entry.next {
            Some(next) => {
                flow.head = next;
                false
            }
            None => true,
        };
        if emptied && let Some(slot) = self.flows.get_mut(usize::from(first)) {
            *slot = None;
        }
        let key_slot = self.keys.get_mut(key_index)?;
        let done = {
            let slot = key_slot.as_mut()?;
            slot.len = slot.len.saturating_sub(1);
            slot.len == 0
        };
        if done {
            *key_slot = None;
        } else if let Some(next) = rest {
            let slot = key_slot.as_mut()?;
            slot.first = next;
            if !emptied {
                // The served flow moves behind the ring's last.
                let last = core::mem::replace(&mut slot.last, first);
                if let Some(Some(last)) = self.flows.get_mut(usize::from(last)).map(Option::as_mut)
                {
                    last.next = Some(first);
                }
                if let Some(Some(served)) =
                    self.flows.get_mut(usize::from(first)).map(Option::as_mut)
                {
                    served.next = None;
                }
            }
        }
        self.len = self.len.saturating_sub(1);
        Some(entry.value)
    }

    /// Inspect each outer queue once, in stable metadata-slot order.
    pub fn heads(&self) -> impl Iterator<Item = (K, &T)> {
        self.keys
            .iter()
            .flatten()
            .filter_map(|slot| self.head_of(slot).map(|value| (slot.key, value)))
    }

    /// The value at the head of `slot`'s first flow.
    fn head_of(&self, slot: &Key<K>) -> Option<&T> {
        let flow = self.flows.get(usize::from(slot.first))?.as_ref()?;
        self.entries
            .get(usize::from(flow.head))?
            .as_ref()
            .map(|entry| &entry.value)
    }

    /// Borrow the next selected inner-flow packet and total outer backlog.
    pub fn head(&self, key: K) -> Option<(&T, usize)> {
        let (_, slot) = self.key(key)?;
        Some((self.head_of(slot)?, slot.len))
    }

    /// First nonempty key strictly after `after`, wrapping to the lowest key.
    pub fn next_head_after(&self, after: Option<K>) -> Option<(K, &T, usize)>
    where
        K: Ord,
    {
        let slot = self
            .keys
            .iter()
            .flatten()
            .min_by_key(|slot| (after.is_some_and(|after| slot.key <= after), slot.key))?;
        Some((slot.key, self.head_of(slot)?, slot.len))
    }

    /// Enumerate outer keys, once per key regardless of its inner flow count.
    pub fn next_key(&self, cursor: &mut usize) -> Option<K> {
        let start = cursor.checked_rem(CAPACITY)?;
        let (slot, key) = self
            .keys
            .iter()
            .enumerate()
            .filter_map(|(slot, key)| key.as_ref().map(|key| (slot, key.key)))
            .min_by_key(|(slot, _)| (*slot < start, *slot))?;
        // The cursor is modular, including externally supplied values.
        *cursor = slot.wrapping_add(1).checked_rem(CAPACITY)?;
        Some(key)
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
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    clippy::arithmetic_side_effects
)]
mod tests;
