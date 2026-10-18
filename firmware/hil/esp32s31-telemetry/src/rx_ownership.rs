//! Bounded RX allocation lifetime accounting. No payloads or packet logs.
//!
//! Live append is a software publication frontier, not a hardware-fetch proof.
//! Completed samples may cross a measurement boundary; `carry_in` records how
//! many allocations were already detached/released when counters were reset.

use oer_esp32s31_ieee80211_dma::rx_observation::RxOwnershipEdge;

#[derive(Clone, Copy)]
enum Slot {
    Idle,
    Detached(u64),
    Released(u64),
}

/// Completed interval statistics in microseconds, not execution-time bounds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Intervals {
    pub samples: u64,
    pub total_micros: u64,
    pub maximum_micros: u64,
}

impl Intervals {
    fn record(&mut self, elapsed: u64) -> Option<()> {
        self.samples = self.samples.checked_add(1)?;
        self.total_micros = self.total_micros.checked_add(elapsed)?;
        self.maximum_micros = self.maximum_micros.max(elapsed);
        Some(())
    }
}

/// Snapshot includes unfinished ownership, never imputes zero-length samples.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    pub hold: Intervals,
    pub returned_to_append: Intervals,
    /// Subsets of the above totals whose interval started before `begin`.
    pub carry_hold: Intervals,
    pub carry_returned_to_append: Intervals,
    pub held: usize,
    pub returned: usize,
    pub maximum_held: usize,
    pub maximum_returned: usize,
    pub carry_in: usize,
    pub reclaimed_while_stopped: usize,
    /// Sticky across windows: missing edges/clock reversal/overflow invalidate evidence.
    pub invalid: bool,
}

pub struct Tracker<const COUNT: usize> {
    arena: Option<usize>,
    slots: [Slot; COUNT],
    carried: [bool; COUNT],
    snapshot: Snapshot,
}

impl<const COUNT: usize> Default for Tracker<COUNT> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const COUNT: usize> Tracker<COUNT> {
    pub const fn new() -> Self {
        Self {
            arena: None,
            slots: [Slot::Idle; COUNT],
            carried: [false; COUNT],
            snapshot: Snapshot {
                hold: Intervals {
                    samples: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                returned_to_append: Intervals {
                    samples: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                carry_hold: Intervals {
                    samples: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                carry_returned_to_append: Intervals {
                    samples: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                held: 0,
                returned: 0,
                maximum_held: 0,
                maximum_returned: 0,
                carry_in: 0,
                reclaimed_while_stopped: 0,
                invalid: false,
            },
        }
    }

    /// Start a counter window without discarding outstanding allocation owners.
    pub fn begin(&mut self) {
        for (carried, slot) in self.carried.iter_mut().zip(&self.slots) {
            *carried = !matches!(slot, Slot::Idle);
        }
        self.snapshot = Snapshot {
            held: self.snapshot.held,
            returned: self.snapshot.returned,
            maximum_held: self.snapshot.held,
            maximum_returned: self.snapshot.returned,
            carry_in: self.snapshot.held + self.snapshot.returned,
            invalid: self.snapshot.invalid,
            ..Snapshot::default()
        };
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot
    }

    pub fn observe(&mut self, arena: usize, buffer: usize, edge: RxOwnershipEdge, now: u64) {
        if self.arena.is_some_and(|existing| existing != arena) || buffer >= COUNT {
            self.snapshot.invalid = true;
            return;
        }
        self.arena = Some(arena);
        let previous = self.slots[buffer];
        let carried = self.carried[buffer];
        use RxOwnershipEdge::*;
        match (previous, edge) {
            (Slot::Idle, Detached) => {
                self.slots[buffer] = Slot::Detached(now);
                self.carried[buffer] = false;
                self.snapshot.held += 1;
                self.snapshot.maximum_held = self.snapshot.maximum_held.max(self.snapshot.held);
            }
            (Slot::Detached(start), Released) => {
                self.slots[buffer] = Slot::Released(now);
                self.carried[buffer] = false;
                self.snapshot.held -= 1;
                self.snapshot.returned += 1;
                self.snapshot.maximum_returned =
                    self.snapshot.maximum_returned.max(self.snapshot.returned);
                if now
                    .checked_sub(start)
                    .and_then(|elapsed| self.snapshot.hold.record(elapsed))
                    .is_none()
                {
                    self.snapshot.invalid = true;
                }
                if carried
                    && now
                        .checked_sub(start)
                        .and_then(|elapsed| self.snapshot.carry_hold.record(elapsed))
                        .is_none()
                {
                    self.snapshot.invalid = true;
                }
            }
            (Slot::Released(start), Republished) => {
                self.slots[buffer] = Slot::Idle;
                self.snapshot.returned -= 1;
                if now
                    .checked_sub(start)
                    .and_then(|elapsed| self.snapshot.returned_to_append.record(elapsed))
                    .is_none()
                {
                    self.snapshot.invalid = true;
                }
                if carried
                    && now
                        .checked_sub(start)
                        .and_then(|elapsed| self.snapshot.carry_returned_to_append.record(elapsed))
                        .is_none()
                {
                    self.snapshot.invalid = true;
                }
            }
            (Slot::Released(_), ReclaimedWhileStopped) => {
                self.slots[buffer] = Slot::Idle;
                self.snapshot.returned -= 1;
                if let Some(count) = self.snapshot.reclaimed_while_stopped.checked_add(1) {
                    self.snapshot.reclaimed_while_stopped = count;
                } else {
                    self.snapshot.invalid = true;
                }
            }
            // Ring-owned discards and initial preparation never detached.
            (Slot::Idle, Republished | ReclaimedWhileStopped) => {}
            _ => self.snapshot.invalid = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RxOwnershipEdge::*;

    #[test]
    fn out_of_order_return_separates_retention_from_head_of_line_wait() {
        let mut t = Tracker::<2>::new();
        t.observe(1, 0, Detached, 10);
        t.observe(1, 1, Detached, 11);
        t.observe(1, 1, Released, 12);
        t.observe(1, 0, Released, 20);
        t.observe(1, 0, Republished, 25);
        t.observe(1, 1, Republished, 26);
        let s = t.snapshot();
        assert_eq!(
            s.hold,
            Intervals {
                samples: 2,
                total_micros: 11,
                maximum_micros: 10
            }
        );
        assert_eq!(
            s.returned_to_append,
            Intervals {
                samples: 2,
                total_micros: 19,
                maximum_micros: 14
            }
        );
        assert_eq!(
            (s.held, s.returned, s.maximum_held, s.maximum_returned),
            (0, 0, 2, 2)
        );
        assert!(!s.invalid);
    }

    #[test]
    fn window_preserves_pending_owners_and_stopped_reclaim_is_not_append() {
        let mut t = Tracker::<2>::new();
        t.observe(1, 0, Detached, 10);
        t.observe(1, 1, Detached, 11);
        t.observe(1, 1, Released, 12);
        t.begin();
        t.observe(1, 1, ReclaimedWhileStopped, 100);
        assert_eq!(t.snapshot().carry_in, 2);
        assert_eq!(t.snapshot().held, 1);
        assert_eq!(t.snapshot().returned_to_append.samples, 0);
        assert_eq!(t.snapshot().reclaimed_while_stopped, 1);
        t.observe(1, 0, Released, 101);
        assert_eq!(t.snapshot().hold.maximum_micros, 91);
        assert_eq!(t.snapshot().carry_hold.maximum_micros, 91);
        t.observe(1, 0, Republished, 110);
        assert_eq!(t.snapshot().returned_to_append.maximum_micros, 9);
        assert_eq!(t.snapshot().carry_returned_to_append.samples, 0);
    }

    #[test]
    fn pre_window_returned_buffer_has_its_own_append_subset() {
        let mut t = Tracker::<1>::new();
        t.observe(1, 0, Detached, 10);
        t.observe(1, 0, Released, 20);
        t.begin();
        t.observe(1, 0, Republished, 100);
        assert_eq!(
            t.snapshot().returned_to_append,
            t.snapshot().carry_returned_to_append
        );
        assert_eq!(t.snapshot().carry_returned_to_append.maximum_micros, 80);
        assert_eq!(t.snapshot().hold.samples, 0);
    }

    #[test]
    fn saturated_interval_never_wraps_into_valid_evidence() {
        let mut t = Tracker::<1>::new();
        t.snapshot.hold.total_micros = u64::MAX;
        t.observe(1, 0, Detached, 1);
        t.observe(1, 0, Released, 2);
        assert!(t.snapshot().invalid);
        assert_eq!(t.snapshot().hold.total_micros, u64::MAX);
        t.begin();
        assert!(t.snapshot().invalid);
    }

    #[test]
    fn invalid_identity_missing_edges_and_clock_reversal_stay_visible() {
        for (arena, buffer, edge, time) in [
            (2, 0, Released, 20),
            (1, 2, Released, 20),
            (1, 0, Republished, 20),
            (1, 0, Released, 9),
        ] {
            let mut t = Tracker::<2>::new();
            t.observe(1, 0, Detached, 10);
            t.observe(arena, buffer, edge, time);
            assert!(t.snapshot().invalid);
            t.begin();
            assert!(t.snapshot().invalid);
        }
    }
}
