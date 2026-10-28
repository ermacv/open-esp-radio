//! Hierarchical queues sharing one bounded owner arena.
//!
//! Packet and inner-flow metadata live together: an occupied entry always
//! owns its value. Outer key slots retain selection order, without links
//! to separately optional owners.
//! The caller selects an outer key; each take serves the next inner flow.
//! Operations scan the bounded arena, without inspecting packet bytes.

struct Entry<K, T, F> {
    key: K,
    flow: F,
    value: T,
    turn: usize,
    first: bool,
}

/// One owner arena, with outer queues of round-robin inner FIFOs.
///
/// A free owner slot always permits publication, including a new outer key
/// or flow. Default `F = ()` gives one FIFO per key. Entries remain in
/// admission order; bounded flow ranks select the next inner FIFO.
/// This policy does not account for bytes, airtime or admission fairness.
pub struct TxQueues<K, T, const CAPACITY: usize, F = ()> {
    entries: [Option<Entry<K, T, F>>; CAPACITY],
    keys: [Option<K>; CAPACITY],
}

impl<K: Copy + Eq, T, const CAPACITY: usize, F: Copy + Eq> TxQueues<K, T, CAPACITY, F> {
    /// An empty arena with capacity checked at compile time.
    ///
    /// ```compile_fail,E0080
    /// use oer_embassy_net::datapath::TxQueues;
    /// let _ = TxQueues::<u8, u8, 0>::new();
    /// ```
    pub const fn new() -> Self {
        const {
            if CAPACITY == 0 {
                panic!("TX queues need at least one owner slot");
            }
        }
        Self {
            entries: [const { None }; CAPACITY],
            keys: [None; CAPACITY],
        }
    }

    pub fn len(&self) -> usize {
        self.entries.iter().flatten().count()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(Option::is_none)
    }
    pub fn is_full(&self) -> bool {
        self.entries.iter().all(Option::is_some)
    }

    /// Total frames across every flow of the selected outer key.
    pub fn len_for(&self, key: K) -> usize {
        self.entries
            .iter()
            .flatten()
            .filter(|entry| entry.key == key)
            .count()
    }

    /// Append to one flow, returning the unchanged owner on exhaustion.
    /// Stable queue slots preserve outer cursor order across packet removal.
    pub fn push_flow(&mut self, key: K, flow: F, value: T) -> Result<(), T> {
        let Some(queue_slot) = self
            .keys
            .iter_mut()
            .filter(|slot| slot.is_none_or(|existing| existing == key))
            .min_by_key(|slot| slot.is_none())
        else {
            return Err(value);
        };
        let existing = self
            .entries
            .iter()
            .flatten()
            .find(|entry| entry.key == key && entry.flow == flow);
        let first = existing.is_none();
        // One first marker per flow; new flows follow every existing turn.
        let turn = existing.map_or_else(
            || {
                self.entries
                    .iter()
                    .flatten()
                    .filter(|entry| entry.key == key && entry.first)
                    .count()
            },
            |entry| entry.turn,
        );
        let Some(slot) = self.entries.iter_mut().find(|slot| slot.is_none()) else {
            return Err(value);
        };
        *queue_slot = Some(key);
        *slot = Some(Entry {
            key,
            flow,
            value,
            turn,
            first,
        });
        Ok(())
    }

    /// Serve the oldest packet of the next flow, then rotate that flow.
    /// Shifting the remaining slots preserves admission order for every key.
    pub fn pop(&mut self, key: K) -> Option<T> {
        let turn = self
            .entries
            .iter()
            .flatten()
            .filter(|entry| entry.key == key)
            .map(|entry| entry.turn)
            .min()?;
        let flow_count = self
            .entries
            .iter()
            .flatten()
            .filter(|entry| entry.key == key && entry.first)
            .count();
        let mut slots = self.entries.iter_mut();
        let mut hole = slots.find(|slot| {
            slot.as_ref()
                .is_some_and(|entry| entry.key == key && entry.turn == turn)
        })?;
        let entry = hole.take()?;
        for next in slots {
            core::mem::swap(hole, next);
            hole = next;
        }
        if let Some(next) = self
            .entries
            .iter_mut()
            .flatten()
            .find(|next| next.key == key && next.flow == entry.flow)
        {
            next.first = true;
        }
        for next in self
            .entries
            .iter_mut()
            .flatten()
            .filter(|next| next.key == key)
        {
            if next.flow == entry.flow {
                // The selected entry proves flow_count >= 1. Ranks remain
                // below the number of flows, which cannot exceed CAPACITY.
                next.turn = flow_count.saturating_sub(1);
            } else if next.turn > turn {
                // This is a bounded rank decrement, never a truncated value.
                next.turn = next.turn.saturating_sub(1);
            }
        }
        if !self.entries.iter().flatten().any(|next| next.key == key)
            && let Some(slot) = self.keys.iter_mut().find(|slot| **slot == Some(key))
        {
            *slot = None;
        }
        Some(entry.value)
    }

    /// Inspect each outer queue once, in stable metadata-slot order.
    pub fn heads(&self) -> impl Iterator<Item = (K, &T)> {
        self.keys
            .iter()
            .flatten()
            .filter_map(|key| self.head(*key).map(|(value, _)| (*key, value)))
    }

    /// Borrow the next selected inner-flow packet and total outer backlog.
    pub fn head(&self, key: K) -> Option<(&T, usize)> {
        let entry = self
            .entries
            .iter()
            .flatten()
            .filter(|entry| entry.key == key)
            .min_by_key(|entry| entry.turn)?;
        Some((&entry.value, self.len_for(key)))
    }

    /// First nonempty key strictly after `after`, wrapping to the lowest key.
    pub fn next_head_after(&self, after: Option<K>) -> Option<(K, &T, usize)>
    where
        K: Ord,
    {
        let entry = self.entries.iter().flatten().min_by_key(|entry| {
            (
                after.is_some_and(|after| entry.key <= after),
                entry.key,
                entry.turn,
            )
        })?;
        Some((entry.key, &entry.value, self.len_for(entry.key)))
    }

    /// Enumerate outer keys, once per key regardless of its inner flow count.
    pub fn next_key(&self, cursor: &mut usize) -> Option<K> {
        let start = cursor.checked_rem(CAPACITY)?;
        let (slot, key) = self
            .keys
            .iter()
            .enumerate()
            .filter_map(|(slot, key)| key.map(|key| (slot, key)))
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
