//! The network core's switchable diagnostics (docs/architecture.md,
//! "Switchable network-core diagnostics"): what the hot path's work was while
//! a reader had them on, switched on, off and reset at runtime.
//!
//! The switch's side ([`DiagnosticsCounter::switch`],
//! [`DiagnosticsCounter::reset`]) only writes a control word. The network's
//! task is the counts' one writer: at each turn, and at each pass of its wait
//! that takes returns, it asks [`DiagnosticsCounter::check`], one load of the
//! control word and one of the reset it last applied; it zeroes the counts
//! there when a reset was asked since, and counts only while on. So a switch
//! or a reset takes effect at the network's next turn. A reader takes each
//! word on its own, on the network's executor between its polls, where each
//! word, a 64-bit sum's halves too, is whole. No count reads a clock; the
//! stages' time is the writer's clock, in its units.

use core::sync::atomic::{AtomicU32, Ordering};

/// The stages a turn's time is split into, and the wait's return servicing.
pub const STAGES: usize = 7;

/// What the hot path did while on, as the network's task counted it. Each
/// count wraps at 2^32, each stage's time at 2^64.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counts {
    pub turns: u32,
    pub collections: u32,
    pub walked: u32,
    pub told: u32,
    pub continued: u32,
    pub stopped_spent: u32,
    pub stopped_changed: u32,
    pub stopped_grant: u32,
    pub stopped_account: u32,
    pub return_batches: u32,
    pub blocks_returned: u32,
    pub credits_restored: u32,
    pub no_credit: u32,
    pub no_credit_loans: u32,
    pub no_credit_unreturned: u32,
    pub materialised: u32,
    /// The writer's clock spent in each stage: the turn's intake, returns,
    /// receive, notices, publish and settle, then the wait's returns.
    pub stage_ticks: [u64; STAGES],
}

/// The words of [`Counts`] but the stages', in field order.
const COUNTS: usize = 16;

impl Counts {
    const fn words(&self) -> [u32; COUNTS] {
        [
            self.turns,
            self.collections,
            self.walked,
            self.told,
            self.continued,
            self.stopped_spent,
            self.stopped_changed,
            self.stopped_grant,
            self.stopped_account,
            self.return_batches,
            self.blocks_returned,
            self.credits_restored,
            self.no_credit,
            self.no_credit_loans,
            self.no_credit_unreturned,
            self.materialised,
        ]
    }

    const fn from_words(words: [u32; COUNTS], stage_ticks: [u64; STAGES]) -> Self {
        let [
            turns,
            collections,
            walked,
            told,
            continued,
            stopped_spent,
            stopped_changed,
            stopped_grant,
            stopped_account,
            return_batches,
            blocks_returned,
            credits_restored,
            no_credit,
            no_credit_loans,
            no_credit_unreturned,
            materialised,
        ] = words;
        Self {
            turns,
            collections,
            walked,
            told,
            continued,
            stopped_spent,
            stopped_changed,
            stopped_grant,
            stopped_account,
            return_batches,
            blocks_returned,
            credits_restored,
            no_credit,
            no_credit_loans,
            no_credit_unreturned,
            materialised,
            stage_ticks,
        }
    }

    /// The counts from `start` to these, each wrapping: a window's, when no
    /// reset fell between them.
    #[must_use]
    pub fn since(&self, start: &Self) -> Self {
        let now = self.words();
        let then = start.words();
        Self::from_words(
            core::array::from_fn(|index| match (now.get(index), then.get(index)) {
                (Some(now), Some(then)) => now.wrapping_sub(*then),
                _ => 0,
            }),
            core::array::from_fn(|stage| {
                match (self.stage_ticks.get(stage), start.stage_ticks.get(stage)) {
                    (Some(now), Some(then)) => now.wrapping_sub(*then),
                    _ => 0,
                }
            }),
        )
    }
}

/// A reading of the diagnostics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetworkDiagnostics {
    /// Whether they were asked to be on.
    pub on: bool,
    /// How often they were switched, on or off, since boot.
    pub switches: u32,
    /// The resets the writer has applied since boot: the counts are what
    /// it counted since the last of them.
    pub resets: u32,
    pub counts: Counts,
}

impl NetworkDiagnostics {
    /// What was counted from `start` to this reading, if it stands for a
    /// whole window: on at both readings, neither switched nor reset
    /// between them. `None` otherwise: such a difference counts no window.
    #[must_use]
    pub fn window(&self, start: &Self) -> Option<Counts> {
        (self.on && start.on && self.switches == start.switches && self.resets == start.resets)
            .then(|| self.counts.since(&start.counts))
    }
}

/// The control word's bit that says on; the bits above count the resets
/// asked for.
const ON: u32 = 1;
const RESET: u32 = 2;

/// The diagnostics' counts, published word by word, and their switch.
pub struct DiagnosticsCounter {
    /// [`ON`], and the resets asked for above it; the switch's side writes
    /// it.
    control: AtomicU32,
    /// Switches that changed it.
    switches: AtomicU32,
    /// The reset count the writer last applied.
    applied: AtomicU32,
    counts: [AtomicU32; COUNTS],
    /// Each stage's sum as its low and its high half.
    halves: [AtomicU32; 2 * STAGES],
}

impl DiagnosticsCounter {
    pub const fn new() -> Self {
        Self {
            control: AtomicU32::new(0),
            switches: AtomicU32::new(0),
            applied: AtomicU32::new(0),
            counts: [const { AtomicU32::new(0) }; COUNTS],
            halves: [const { AtomicU32::new(0) }; 2 * STAGES],
        }
    }

    /// Ask them on or off, from the network's next turn; whether they were
    /// asked on before.
    pub fn switch(&self, on: bool) -> bool {
        let was = if on {
            self.control.fetch_or(ON, Ordering::AcqRel)
        } else {
            self.control.fetch_and(!ON, Ordering::AcqRel)
        };
        let was_on = was & ON != 0;
        if was_on != on {
            self.switches.fetch_add(1, Ordering::AcqRel);
        }
        was_on
    }

    /// Ask the counts zeroed, at the network's next turn, on or off.
    pub fn reset(&self) {
        self.control.fetch_add(RESET, Ordering::AcqRel);
    }

    /// The writer's check, at each turn and each pass that takes returns:
    /// apply a reset asked since the last, and say whether to count.
    pub fn check(&self) -> bool {
        let control = self.control.load(Ordering::Acquire);
        let asked = control >> 1;
        if asked != self.applied.load(Ordering::Relaxed) {
            for count in self.counts.iter().chain(self.halves.iter()) {
                count.store(0, Ordering::Relaxed);
            }
            self.applied.store(asked, Ordering::Release);
        }
        control & ON != 0
    }

    /// The writer adds what one turn or pass counted.
    pub fn add(&self, counted: &Counts) {
        for (count, by) in self.counts.iter().zip(counted.words()) {
            // One writer: a load and a store.
            count.store(
                count.load(Ordering::Relaxed).wrapping_add(by),
                Ordering::Relaxed,
            );
        }
        for (stage, by) in counted.stage_ticks.iter().enumerate() {
            let halves = stage.checked_mul(2).and_then(|low| {
                Some((self.halves.get(low)?, self.halves.get(low.checked_add(1)?)?))
            });
            if let Some((low, high)) = halves {
                let was = u64::from(low.load(Ordering::Relaxed))
                    | (u64::from(high.load(Ordering::Relaxed)) << 32);
                let now = was.wrapping_add(*by);
                // Halves of a 64-bit sum.
                low.store(now as u32, Ordering::Relaxed);
                high.store((now >> 32) as u32, Ordering::Relaxed);
            }
        }
    }

    /// The diagnostics now.
    pub fn read(&self) -> NetworkDiagnostics {
        let load = |word: Option<&AtomicU32>| word.map_or(0, |word| word.load(Ordering::Relaxed));
        let stage = |stage: usize| {
            let low = stage.checked_mul(2);
            let high = low.and_then(|low| low.checked_add(1));
            u64::from(load(low.and_then(|low| self.halves.get(low))))
                | (u64::from(load(high.and_then(|high| self.halves.get(high)))) << 32)
        };
        NetworkDiagnostics {
            on: self.control.load(Ordering::Acquire) & ON != 0,
            switches: self.switches.load(Ordering::Acquire),
            resets: self.applied.load(Ordering::Acquire),
            counts: Counts::from_words(
                core::array::from_fn(|index| load(self.counts.get(index))),
                core::array::from_fn(stage),
            ),
        }
    }
}

impl Default for DiagnosticsCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
