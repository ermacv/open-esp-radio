//! A recording observer for host tests.

use core::cell::Cell;

use oer_trace::{Event, Observer};

use crate::Observation;

/// Keeps the first `N` observations it receives, in order; events of other
/// vocabularies are ignored. With `cfg(oer_trace)` off it receives nothing.
pub struct Recorder<const N: usize> {
    observations: Cell<[Option<Observation>; N]>,
    overflowed: Cell<bool>,
}

impl<const N: usize> Recorder<N> {
    pub const fn new() -> Self {
        Self {
            observations: Cell::new([None; N]),
            overflowed: Cell::new(false),
        }
    }

    /// The observations kept, oldest first.
    pub fn observations(&self) -> impl Iterator<Item = Observation> {
        self.observations.get().into_iter().flatten()
    }

    /// Whether an observation arrived after `N` were kept.
    pub fn overflowed(&self) -> bool {
        self.overflowed.get()
    }
}

impl<const N: usize> Default for Recorder<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Observer for Recorder<N> {
    fn event<E: Event + Copy + 'static>(&self, event: E) {
        let Some(observation) = Observation::of(&event) else {
            return;
        };
        let mut observations = self.observations.get();
        match observations.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(observation),
            None => self.overflowed.set(true),
        }
        self.observations.set(observations);
    }
}
