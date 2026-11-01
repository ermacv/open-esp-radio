//! The executors' CPU since boot, from their activity observations
//! (`oer_espressif_executor::observation::Activity`), which only an
//! `oer_trace` build emits.
//!
//! Each task set of the image selects the observer of its own slot, so a
//! slot has one writer: its executor's context. The writer publishes under a
//! sequence number, odd while it writes; a reader that sees it change
//! samples again after yielding, and never waits; it may run on either
//! hart. Work spans include the executor's preemption, so the spans of two
//! executors overlap and do not add up. The boot hart's idle spans (its
//! thread executor's masked WFI) give its utilisation; hart 1 runs one
//! interrupt-mode executor and otherwise waits for interrupts, so its idle
//! time is all its time but that executor's work. Activity times wrap at
//! 2^32 µs; a slot keeps its totals in 64 bits.

use core::sync::atomic::{AtomicU32, Ordering, fence};

use oer_services_load::{ExecutorLoad, HartLoad, Load, MAX_EXECUTORS, MAX_HARTS};

/// One slot per task set: the boot hart's application executor (P1, its
/// thread executor) and radio executor (P2), and hart 1's network executor
/// (P2, the only executor of that hart).
pub(crate) const SLOTS: usize = 3;
pub(crate) const APPLICATION: usize = 0;
pub(crate) const RADIO: usize = 1;
pub(crate) const NETWORK: usize = 2;
/// The harts the slots' executors run on: the boot hart and hart 1.
const HARTS: usize = 2;
/// Whether a slot's executor is its hart's only one, polled in its
/// interrupt while the hart's main waits for interrupts: that hart's idle
/// time is the rest of its time.
const SOLE: [bool; SLOTS] = [false, false, true];
const _: () = {
    if SLOTS > MAX_EXECUTORS
        || HARTS > MAX_HARTS
        || APPLICATION == RADIO
        || RADIO == NETWORK
        || APPLICATION == NETWORK
        || NETWORK >= SLOTS
        || RADIO >= SLOTS
    {
        panic!("every slot's executor and hart fit one CPU report");
    }
};

const OPEN_NONE: u32 = 0;
const OPEN_WORK: u32 = 1;
const OPEN_IDLE: u32 = 2;
const SEEN_WORK: u8 = 1;
const SEEN_IDLE: u8 = 2;

/// An executor's activity boundary.
#[cfg(any(oer_trace, test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Phase {
    WorkStart,
    WorkEnd,
    IdleStart,
    IdleEnd,
}

/// One boundary at `at_us`, wrapping activity microseconds.
#[cfg(any(oer_trace, test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Boundary {
    pub at_us: u32,
    pub executor: u8,
    pub hart: u8,
    pub level: u8,
    pub phase: Phase,
}

/// What one slot accumulated: `None` for a kind of span it never saw.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Sample {
    pub executor: u8,
    pub hart: u8,
    pub level: u8,
    pub work_us: Option<u64>,
    pub idle_us: Option<u64>,
}

/// The slot's writer was writing while it was sampled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Concurrent;

/// The cumulative work and idle microseconds of one executor.
pub(crate) struct CpuCounter {
    sequence: AtomicU32,
    /// Low and high words.
    work: [AtomicU32; 2],
    idle: [AtomicU32; 2],
    /// The start of the open span.
    start: AtomicU32,
    open: AtomicU32,
    /// Executor, hart, level and the kinds of span seen.
    context: AtomicU32,
}

impl CpuCounter {
    pub(crate) const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            work: [AtomicU32::new(0), AtomicU32::new(0)],
            idle: [AtomicU32::new(0), AtomicU32::new(0)],
            start: AtomicU32::new(0),
            open: AtomicU32::new(OPEN_NONE),
            context: AtomicU32::new(0),
        }
    }

    /// Account `boundary`; only the slot's executor calls it. An end
    /// without its start adds nothing.
    #[cfg(any(oer_trace, test))]
    pub(crate) fn observe(&self, boundary: Boundary) {
        let sequence = self.sequence.load(Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        let [_, _, _, seen] = self.context.load(Ordering::Relaxed).to_le_bytes();
        let open = self.open.load(Ordering::Relaxed);
        let span = || {
            boundary
                .at_us
                .wrapping_sub(self.start.load(Ordering::Relaxed))
        };
        let seen = match boundary.phase {
            Phase::WorkStart | Phase::IdleStart => {
                self.start.store(boundary.at_us, Ordering::Relaxed);
                let (kind, open) = match boundary.phase {
                    Phase::WorkStart => (SEEN_WORK, OPEN_WORK),
                    _ => (SEEN_IDLE, OPEN_IDLE),
                };
                self.open.store(open, Ordering::Relaxed);
                seen | kind
            }
            Phase::WorkEnd | Phase::IdleEnd => {
                match (boundary.phase, open) {
                    (Phase::WorkEnd, OPEN_WORK) => add(&self.work, span()),
                    (Phase::IdleEnd, OPEN_IDLE) => add(&self.idle, span()),
                    _ => {}
                }
                self.open.store(OPEN_NONE, Ordering::Relaxed);
                seen
            }
        };
        self.context.store(
            u32::from_le_bytes([boundary.executor, boundary.hart, boundary.level, seen]),
            Ordering::Relaxed,
        );
        self.sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
    }

    /// The totals at `now_us` (activity microseconds), the open span
    /// included.
    pub(crate) fn sample(&self, now_us: u32) -> Result<Sample, Concurrent> {
        let sequence = self.sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 {
            return Err(Concurrent);
        }
        let work = total(&self.work);
        let idle = total(&self.idle);
        let start = self.start.load(Ordering::Relaxed);
        let open = self.open.load(Ordering::Relaxed);
        let [executor, hart, level, seen] = self.context.load(Ordering::Relaxed).to_le_bytes();
        fence(Ordering::Acquire);
        if self.sequence.load(Ordering::Relaxed) != sequence {
            return Err(Concurrent);
        }
        let span = u64::from(now_us.wrapping_sub(start));
        let with_open = |total: u64, kind| {
            if open == kind {
                total.wrapping_add(span)
            } else {
                total
            }
        };
        Ok(Sample {
            executor,
            hart,
            level,
            work_us: (seen & SEEN_WORK != 0).then(|| with_open(work, OPEN_WORK)),
            idle_us: (seen & SEEN_IDLE != 0).then(|| with_open(idle, OPEN_IDLE)),
        })
    }
}

#[cfg(any(oer_trace, test))]
fn add(total: &[AtomicU32; 2], span: u32) {
    let [low, high] = total;
    let sum = (u64::from(high.load(Ordering::Relaxed)) << 32
        | u64::from(low.load(Ordering::Relaxed)))
    .wrapping_add(u64::from(span));
    low.store(sum as u32, Ordering::Relaxed);
    high.store((sum >> 32) as u32, Ordering::Relaxed);
}

fn total(total: &[AtomicU32; 2]) -> u64 {
    let [low, high] = total;
    u64::from(high.load(Ordering::Relaxed)) << 32 | u64::from(low.load(Ordering::Relaxed))
}

/// The CPU report at image microsecond `now_us` of every slot's sample: a
/// hart for each slot that saw idle or is its hart's sole executor, an
/// executor for each that saw work. A sole executor's hart idles for all of
/// `now_us` but that executor's work: between two reports only the
/// difference counts.
pub(crate) fn load(now_us: u64, samples: &[Sample; SLOTS]) -> Load {
    let mut load = Load {
        now_us,
        ..Load::default()
    };
    for (sample, sole) in samples.iter().zip(SOLE) {
        let idle = match (sample.idle_us, sole, sample.work_us) {
            (Some(idle_us), _, _) => Some(idle_us),
            (None, true, Some(work_us)) => Some(now_us.saturating_sub(work_us)),
            (None, _, _) => None,
        };
        if let Some(idle_us) = idle {
            match load.harts.iter_mut().find(|hart| hart.hart == sample.hart) {
                Some(hart) => hart.idle_us = hart.idle_us.saturating_add(idle_us),
                // HARTS <= MAX_HARTS: a slot's hart always has room.
                None => {
                    let _room = load.harts.push(HartLoad {
                        hart: sample.hart,
                        idle_us,
                    });
                }
            }
        }
        if let Some(work_us) = sample.work_us {
            // SLOTS <= MAX_EXECUTORS: a slot's executor always has room.
            let _room = load.executors.push(ExecutorLoad {
                executor: sample.executor,
                hart: sample.hart,
                level: sample.level,
                work_us,
            });
        }
    }
    load
}

/// Every slot's counter.
#[cfg(target_arch = "riscv32")]
pub(crate) static COUNTERS: [CpuCounter; SLOTS] = [const { CpuCounter::new() }; SLOTS];

/// The observer a task set selects for slot `SLOT`.
#[cfg(all(target_arch = "riscv32", oer_trace))]
pub(crate) struct Observer<const SLOT: usize>;

#[cfg(all(target_arch = "riscv32", oer_trace))]
impl<const SLOT: usize> oer_trace::Observer for Observer<SLOT> {
    fn event<E: oer_trace::Event + Copy + 'static>(&self, event: E) {
        use oer_espressif_executor::observation::{Activity, Phase as Executor};
        const {
            if SLOT >= SLOTS {
                panic!("the observer's slot is one of the image's");
            }
        }
        if let (Some(activity), Some(counter)) = (Activity::of(&event), COUNTERS.get(SLOT)) {
            counter.observe(Boundary {
                at_us: activity.at_micros,
                executor: activity.executor,
                hart: activity.hart,
                level: activity.level,
                phase: match activity.phase {
                    Executor::WorkStart => Phase::WorkStart,
                    Executor::WorkEnd => Phase::WorkEnd,
                    Executor::IdleStart => Phase::IdleStart,
                    Executor::IdleEnd => Phase::IdleEnd,
                },
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(at_us: u32, phase: Phase) -> Boundary {
        Boundary {
            at_us,
            executor: 2,
            hart: 0,
            level: 3,
            phase,
        }
    }

    #[test]
    fn a_slot_accumulates_closed_and_open_spans_of_each_kind() {
        let counter = CpuCounter::new();
        assert_eq!(counter.sample(5), Ok(Sample::default()), "nothing seen");
        counter.observe(at(10, Phase::WorkStart));
        counter.observe(at(25, Phase::WorkEnd));
        counter.observe(at(30, Phase::IdleStart));
        counter.observe(at(70, Phase::IdleEnd));
        counter.observe(at(100, Phase::WorkStart));
        let sample = counter.sample(104).unwrap();
        assert_eq!((sample.executor, sample.hart, sample.level), (2, 0, 3));
        assert_eq!(sample.work_us, Some(15 + 4), "the open span counts");
        assert_eq!(sample.idle_us, Some(40));
    }

    #[test]
    fn spans_cross_the_activity_clock_wrap_and_totals_exceed_32_bits() {
        let counter = CpuCounter::new();
        counter.observe(at(u32::MAX - 4, Phase::WorkStart));
        counter.observe(at(5, Phase::WorkEnd));
        assert_eq!(counter.sample(6).unwrap().work_us, Some(10));
        for _ in 0..3 {
            counter.observe(at(0, Phase::WorkStart));
            counter.observe(at(u32::MAX, Phase::WorkEnd));
        }
        assert_eq!(
            counter.sample(6).unwrap().work_us,
            Some(10 + 3 * u64::from(u32::MAX))
        );
    }

    #[test]
    fn an_end_without_its_start_adds_nothing() {
        let counter = CpuCounter::new();
        counter.observe(at(10, Phase::IdleStart));
        counter.observe(at(20, Phase::WorkEnd));
        counter.observe(at(30, Phase::IdleEnd));
        let sample = counter.sample(40).unwrap();
        assert_eq!((sample.work_us, sample.idle_us), (None, Some(0)));
    }

    #[test]
    fn a_sample_during_a_write_is_refused() {
        let counter = CpuCounter::new();
        counter.sequence.store(1, Ordering::Relaxed);
        assert_eq!(counter.sample(0), Err(Concurrent));
    }

    #[test]
    fn the_report_names_harts_by_idle_and_executors_by_work() {
        let application = Sample {
            executor: 0,
            hart: 0,
            level: 0,
            work_us: Some(300),
            idle_us: Some(700),
        };
        let radio = Sample {
            executor: 2,
            hart: 0,
            level: 2,
            work_us: Some(50),
            idle_us: None,
        };
        let load = load(1000, &[application, radio, Sample::default()]);
        assert_eq!(load.now_us, 1000);
        assert_eq!(
            load.harts.as_slice(),
            &[HartLoad {
                hart: 0,
                idle_us: 700
            }]
        );
        assert_eq!(
            load.executors.as_slice(),
            &[
                ExecutorLoad {
                    executor: 0,
                    hart: 0,
                    level: 0,
                    work_us: 300
                },
                ExecutorLoad {
                    executor: 2,
                    hart: 0,
                    level: 2,
                    work_us: 50
                }
            ]
        );
        let unobserved = load_of_nothing();
        assert!(unobserved.harts.is_empty() && unobserved.executors.is_empty());
    }

    fn load_of_nothing() -> Load {
        load(5, &[Sample::default(); SLOTS])
    }

    #[test]
    fn hart_1_idles_for_all_its_time_but_its_sole_executor_s_work() {
        let network = |work_us| Sample {
            executor: 1,
            hart: 1,
            level: 2,
            work_us: Some(work_us),
            idle_us: None,
        };
        let idle = |load: &Load| {
            load.harts
                .iter()
                .find(|hart| hart.hart == 1)
                .map(|hart| hart.idle_us)
        };
        let before = load(1_000, &[Sample::default(), Sample::default(), network(100)]);
        let after = load(
            3_000,
            &[Sample::default(), Sample::default(), network(1_600)],
        );
        assert_eq!((idle(&before), idle(&after)), (Some(900), Some(1_400)));
        // Between the two reports: 2000 µs, of which 1500 µs work, 500 idle.
        assert_eq!(idle(&after).unwrap() - idle(&before).unwrap(), 500);
        assert_eq!(
            after.executors.as_slice(),
            &[ExecutorLoad {
                executor: 1,
                hart: 1,
                level: 2,
                work_us: 1_600
            }]
        );
        // Before its executor's first work, hart 1 reports nothing.
        let unstarted = Sample {
            work_us: None,
            ..network(0)
        };
        assert_eq!(
            idle(&load(
                10,
                &[Sample::default(), Sample::default(), unstarted]
            )),
            None
        );
    }

    #[test]
    fn the_boot_hart_s_idle_does_not_come_from_its_executors_work() {
        let radio = Sample {
            executor: 2,
            hart: 0,
            level: 2,
            work_us: Some(50),
            idle_us: None,
        };
        let load = load(1000, &[Sample::default(), radio, Sample::default()]);
        assert!(
            load.harts.is_empty(),
            "the radio executor is not hart 0's only one"
        );
    }
}
