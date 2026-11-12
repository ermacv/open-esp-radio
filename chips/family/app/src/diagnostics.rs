//! The network core's switchable diagnostics (docs/architecture.md,
//! "Switchable network-core diagnostics"): what the hot path's work was while
//! a reader had them on, switched on, off and reset at runtime.
//!
//! The switch's side ([`DiagnosticsCounter::switch`],
//! [`DiagnosticsCounter::reset`]) only writes one control word, which holds
//! the state and the count of its changes together. The network's task is
//! the counts' one writer: at each turn, and at each pass of its wait that
//! takes returns, it asks [`DiagnosticsCounter::check`], one load of the
//! control word and one of the word it last applied; it applies a changed
//! word, zeroing the counts when a reset was asked since, and counts only
//! while on. A reading is settled only when the writer had applied the word
//! it read, and a window is whole only between two settled readings of one
//! word.
//!
//! The word's switch and reset counts never wrap: each stops at its end
//! ([`MARKER_SWITCHES`], [`MARKER_RESETS`]), and from then the marker is
//! spent. No window is whole again, one that starts later too, and a reset
//! is refused ([`ResetRefused`]) rather than asked so that the writer could
//! miss it. So no value the marker had while valid comes back. So a switch
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
    pub walked: u32,
    pub told: u32,
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
const COUNTS: usize = 10;

impl Counts {
    const fn words(&self) -> [u32; COUNTS] {
        [
            self.turns,
            self.walked,
            self.told,
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
            walked,
            told,
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
            walked,
            told,
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
    /// The control word at the reading: whether on, and how many switches
    /// and resets were asked before it ([`Self::switches`],
    /// [`Self::resets`]). Every change moves it.
    pub marker: u32,
    /// Whether the writer had applied that control word: no switch or reset
    /// was waiting for its next turn.
    pub settled: bool,
    /// What the writer counted since the last reset it applied.
    pub counts: Counts,
}

impl NetworkDiagnostics {
    /// Switches that changed the state since boot, up to
    /// [`MARKER_SWITCHES`].
    #[must_use]
    pub const fn switches(&self) -> u32 {
        (self.marker & SWITCHES) >> 1
    }

    /// Resets asked since boot, up to [`MARKER_RESETS`].
    #[must_use]
    pub const fn resets(&self) -> u32 {
        self.marker >> 16
    }

    /// Whether the marker is spent: its switch or its reset count reached
    /// its end, so it can no longer tell one change from another, and no
    /// window is whole again, a window that starts after this too.
    #[must_use]
    pub const fn exhausted(&self) -> bool {
        exhausted(self.marker)
    }

    /// What was counted from `start` to this reading, if it stands for a
    /// whole window: on and settled at both readings, with the same control
    /// word, so no switch or reset was asked between them and none was
    /// waiting for the writer, and the marker not spent at either.
    ///
    /// # Errors
    /// [`NoWindow`]: such a difference counts no window.
    pub fn window(&self, start: &Self) -> Result<Counts, NoWindow> {
        if self.exhausted() || start.exhausted() {
            return Err(NoWindow::Exhausted);
        }
        if self.on && self.settled && start.on && start.settled && self.marker == start.marker {
            Ok(self.counts.since(&start.counts))
        } else {
            Err(NoWindow::Changed)
        }
    }
}

/// Why two readings count no window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoWindow {
    /// Off at an end, switched or reset between them, or a change not yet
    /// applied by the writer at an end.
    Changed,
    /// The marker is spent ([`NetworkDiagnostics::exhausted`]).
    Exhausted,
}

/// A reset was not asked: the marker's reset count is at its end, so the
/// writer could not tell this reset from the last. The counts are as they
/// were.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResetRefused;

/// The control word: bit 0 says on, bits 1 to 15 count the switches that
/// changed it, bits 16 to 31 the resets asked for. One word, so a reading
/// takes the state and its change count together. Neither count wraps: each
/// stops at its end, and from then the marker is spent, so no value it had
/// while valid comes back.
const ON: u32 = 1;
const SWITCH: u32 = 1 << 1;
const SWITCHES: u32 = 0x7fff << 1;
const RESET: u32 = 1 << 16;
const RESETS: u32 = 0xffff << 16;

/// The most switches and resets a marker counts; at either, it is spent.
pub const MARKER_SWITCHES: u32 = SWITCHES >> 1;
pub const MARKER_RESETS: u32 = RESETS >> 16;

const fn exhausted(marker: u32) -> bool {
    marker & SWITCHES == SWITCHES || marker & RESETS == RESETS
}

/// The diagnostics' counts, published word by word, and their switch.
pub struct DiagnosticsCounter {
    /// The control word; the switch's side writes it.
    control: AtomicU32,
    /// The control word the writer last applied.
    applied: AtomicU32,
    counts: [AtomicU32; COUNTS],
    /// Each stage's sum as its low and its high half.
    halves: [AtomicU32; 2 * STAGES],
}

impl DiagnosticsCounter {
    pub const fn new() -> Self {
        Self {
            control: AtomicU32::new(0),
            applied: AtomicU32::new(0),
            counts: [const { AtomicU32::new(0) }; COUNTS],
            halves: [const { AtomicU32::new(0) }; 2 * STAGES],
        }
    }

    /// Ask them on or off, from the network's next turn; whether they were
    /// asked on before. A switch that changes the state moves the switch
    /// count in the same word; at its end the count stays there, the marker
    /// spent, and the state still switches.
    pub fn switch(&self, on: bool) -> bool {
        let changed = |control: u32| {
            let was_on = control & ON != 0;
            (was_on != on).then(|| {
                let switches = if control & SWITCHES == SWITCHES {
                    SWITCHES
                } else {
                    control.wrapping_add(SWITCH) & SWITCHES
                };
                (control & !(SWITCHES | ON)) | switches | u32::from(on)
            })
        };
        match self
            .control
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, changed)
        {
            Ok(was) | Err(was) => was & ON != 0,
        }
    }

    /// Ask the counts zeroed, at the network's next turn, on or off.
    ///
    /// # Errors
    /// [`ResetRefused`]: the marker's reset count is at its end; nothing
    /// was asked.
    pub fn reset(&self) -> Result<(), ResetRefused> {
        self.control
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |control| {
                (control & RESETS != RESETS).then(|| control.wrapping_add(RESET))
            })
            .map(|_| ())
            .map_err(|_| ResetRefused)
    }

    /// The writer's check, at each turn and each pass that takes returns:
    /// apply the control word, zeroing the counts if a reset was asked since
    /// the last, and say whether to count.
    pub fn check(&self) -> bool {
        let control = self.control.load(Ordering::Acquire);
        let applied = self.applied.load(Ordering::Relaxed);
        if control != applied {
            if control & RESETS != applied & RESETS {
                for count in self.counts.iter().chain(self.halves.iter()) {
                    count.store(0, Ordering::Relaxed);
                }
            }
            self.applied.store(control, Ordering::Release);
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
        // The control word on both sides of the applied one: a change
        // between them leaves the reading unsettled.
        let before = self.control.load(Ordering::Acquire);
        let applied = self.applied.load(Ordering::Acquire);
        let after = self.control.load(Ordering::Acquire);
        NetworkDiagnostics {
            on: before & ON != 0,
            marker: before,
            settled: before == applied && applied == after,
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
