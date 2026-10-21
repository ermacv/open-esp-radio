//! Bounded snapshots of atomic words on the same-core P1/P2 boundary.

use core::sync::atomic::{AtomicU32, Ordering};

/// Writers of one record are serialized by its endpoint's execution context.
/// P2 never waits for a preempted P1 writer: an incomplete snapshot is None.
/// All payload words are atomic, including across a rejected snapshot.
#[derive(Debug)]
pub(super) struct AtomicWords<const N: usize> {
    generation: AtomicU32,
    words: [AtomicU32; N],
}

impl<const N: usize> AtomicWords<N> {
    pub(super) const fn new() -> Self {
        Self {
            generation: AtomicU32::new(0),
            words: [const { AtomicU32::new(0) }; N],
        }
    }

    pub(super) fn set(&self, words: [u32; N]) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        for (cell, value) in self.words.iter().zip(words) {
            cell.store(value, Ordering::SeqCst);
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub(super) fn get(&self) -> Option<[u32; N]> {
        let before = self.generation.load(Ordering::SeqCst);
        if before & 1 != 0 {
            return None;
        }
        let words = core::array::from_fn(|index| self.words[index].load(Ordering::SeqCst));
        (before == self.generation.load(Ordering::SeqCst)).then_some(words)
    }
}

/// P1 writes PTK activity; P2 writes GTK activity. Initialization and normal
/// retirement run on P2 outside all endpoint loans. Abandonment does not
/// mutate these records: quarantine already prevents every subsequent use.
#[derive(Debug)]
pub(super) struct KeyActivity(AtomicWords<2>);

impl KeyActivity {
    pub(super) const fn new() -> Self {
        Self(AtomicWords::new())
    }
    pub(super) fn set(&self, serial: u64) {
        self.0.set([serial as u32, (serial >> 32) as u32]);
    }
    pub(super) fn matches(&self, serial: u64) -> bool {
        self.0.get() == Some([serial as u32, (serial >> 32) as u32])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interrupted_writer_is_not_a_live_key_and_the_reader_does_not_retry() {
        let activity = KeyActivity::new();
        activity.set(1);
        assert!(activity.matches(1));
        activity.0.generation.fetch_add(1, Ordering::SeqCst);
        activity.0.words[0].store(2, Ordering::SeqCst);
        assert!(!activity.matches(1));
        assert!(!activity.matches(2));
        activity.0.generation.fetch_add(1, Ordering::SeqCst);
        assert!(activity.matches(2));
    }
}
