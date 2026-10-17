//! Each executor's cycles in its idle wait over an explicit window, from
//! which a per-core CPU load follows. No image enables it and nothing reports
//! the load yet.
//!
//! With `idle-accounting`, an executor's run loop reads the 64-bit `mcycle`
//! of its core before and after its `wfi`, with interrupts masked, and adds
//! the difference to that executor's idle counter. The interrupt that ends
//! the wait runs only after the second read, so the counter holds the time
//! the core waited and nothing else: tasks, interrupt handlers and the
//! executor's own polling are busy time. The cost is two 64-bit CSR reads and
//! a single-writer counter update per wait, none per poll or per interrupt.
//!
//! `mcycle` must count while the core waits. The ESP32-S31 leaves
//! `HP_SYSTEM_CPU_WAITI_CONF.CPU_WAIT_MODE_FORCE_ON` at its reset value, 1,
//! which keeps the core clock running in `wfi`; an image that clears it
//! reads every wait as zero idle cycles, and so a load near 100 % on an idle
//! link.
//!
//! A [`Window`] samples every running executor's counter at its beginning
//! and its end, with the calling core's `mcycle` as the window's length:
//! the cores share one clock. A core waiting at a boundary has not yet
//! added its current wait, so the sampler raises that executor's software
//! interrupt and waits until the core has added it: the wait then counts up
//! to the boundary plus its wake-up latency, at the cost of one extra
//! executor poll on that core per boundary. A core that does not answer
//! within a bounded number of attempts is reported as unmeasured.

/// Executors one image runs at most: one per software interrupt.
pub const EXECUTORS: usize = 4;

/// One executor's idle counter at an instant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sample {
    /// The core the executor runs on.
    pub core: usize,
    /// Cycles the executor has waited since boot, wrapping.
    pub idle_cycles: u64,
}

/// One executor's idle cycles over a window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutorIdle {
    pub core: usize,
    pub idle_cycles: u64,
}

/// The idle cycles of every executor over one window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Report {
    /// The window's length in cycles of the core that opened and closed it.
    pub window_cycles: u64,
    /// By software interrupt: `None` for an executor not running at both
    /// boundaries, moved to another core, or not answering at one of them.
    pub executors: [Option<ExecutorIdle>; EXECUTORS],
}

impl Report {
    /// The window between two boundaries: each counter's difference, with
    /// wrapping, since the counters and `mcycle` wrap.
    pub fn between(
        started_cycle: u64,
        start: [Option<Sample>; EXECUTORS],
        ended_cycle: u64,
        end: [Option<Sample>; EXECUTORS],
    ) -> Self {
        Self {
            window_cycles: ended_cycle.wrapping_sub(started_cycle),
            executors: core::array::from_fn(|index| match (start[index], end[index]) {
                (Some(start), Some(end)) if start.core == end.core => Some(ExecutorIdle {
                    core: end.core,
                    idle_cycles: end.idle_cycles.wrapping_sub(start.idle_cycles),
                }),
                _ => None,
            }),
        }
    }

    /// The idle cycles of the executor on `core`, when one was measured.
    pub fn idle_cycles(&self, core: usize) -> Option<u64> {
        self.executors
            .iter()
            .flatten()
            .find(|executor| executor.core == core)
            .map(|executor| executor.idle_cycles)
    }
}

#[cfg(feature = "chip")]
pub use chip::{Window, add_wait};

#[cfg(feature = "chip")]
mod chip {
    use core::marker::PhantomData;

    use portable_atomic::{AtomicBool, AtomicU32, Ordering, fence};

    use super::{EXECUTORS, Report, Sample};

    /// Attempts a sampler makes for a waiting core to add its wait, and for
    /// a consistent read of a counter.
    const ATTEMPTS: u32 = 100_000;

    /// One executor's idle counter. Only its executor's core writes it, with
    /// interrupts masked; samplers on any core read it as a sequence lock.
    #[repr(C)]
    struct Counter {
        /// Set from just before `wfi` until the wait's cycles are taken.
        waiting: AtomicBool,
        /// Odd while the total is being written.
        sequence: AtomicU32,
        low: AtomicU32,
        high: AtomicU32,
    }

    #[repr(transparent)]
    struct Counters([Counter; EXECUTORS]);

    #[allow(unsafe_code, reason = "a zero counter is zero bytes")]
    // SAFETY: `portable_atomic::AtomicBool` and `AtomicU32` have the in-memory
    // representation of `bool` and `u32` (their documented guarantee), so
    // zero bytes are a counter that is not waiting, stable and at zero.
    unsafe impl bytemuck::Zeroable for Counters {}

    oer_memory::zeroed_static! {
        /// Placement: the board linker owns this exported executor-state
        /// section.
        #[used]
        static IDLE: Counters = zeroed in ".critical.bss.embassy_executor";
    }

    impl Counter {
        fn add(&self, cycles: u64) {
            let sequence = self.sequence.load(Ordering::Relaxed);
            self.sequence
                .store(sequence.wrapping_add(1), Ordering::Relaxed);
            fence(Ordering::Release);
            let total = (u64::from(self.high.load(Ordering::Relaxed)) << 32
                | u64::from(self.low.load(Ordering::Relaxed)))
            .wrapping_add(cycles);
            self.low.store(total as u32, Ordering::Relaxed);
            self.high.store((total >> 32) as u32, Ordering::Relaxed);
            self.sequence
                .store(sequence.wrapping_add(2), Ordering::Release);
        }

        fn read(&self) -> Option<u64> {
            for _ in 0..ATTEMPTS {
                let first = self.sequence.load(Ordering::Acquire);
                if first & 1 == 0 {
                    let low = self.low.load(Ordering::Relaxed);
                    let high = self.high.load(Ordering::Relaxed);
                    fence(Ordering::Acquire);
                    if self.sequence.load(Ordering::Relaxed) == first {
                        return Some(u64::from(high) << 32 | u64::from(low));
                    }
                }
                pause();
            }
            None
        }
    }

    #[allow(
        clippy::disallowed_methods,
        reason = "a sampler waits microseconds for another core's counter update; the window \
                  boundary must not move to a later poll"
    )]
    fn pause() {
        core::hint::spin_loop();
    }

    /// The executor `executor`'s wait for an interrupt, its cycles added to
    /// its idle counter. Called with interrupts masked, on the executor's
    /// core only.
    #[inline(always)]
    pub fn add_wait(executor: usize) {
        let counter = &IDLE.0[executor];
        counter.waiting.store(true, Ordering::Release);
        let before = riscv::register::mcycle::read64();
        esp_hal::interrupt::wait_for_interrupt();
        let after = riscv::register::mcycle::read64();
        // Cleared before the update, so a sampler that sees the wait
        // always sees the update that ends it.
        counter.waiting.store(false, Ordering::Release);
        counter.add(after.wrapping_sub(before));
    }

    /// Every running executor's idle counter now.
    fn sample() -> [Option<Sample>; EXECUTORS] {
        core::array::from_fn(|executor| {
            let core = crate::executor::executor_core(executor)?;
            let counter = &IDLE.0[executor];
            let sequence = counter.sequence.load(Ordering::Acquire);
            if counter.waiting.load(Ordering::Acquire) {
                crate::executor::raise(executor);
                let mut answered = false;
                for _ in 0..ATTEMPTS {
                    if counter.sequence.load(Ordering::Acquire) != sequence {
                        answered = true;
                        break;
                    }
                    pause();
                }
                if !answered {
                    return None;
                }
            }
            Some(Sample {
                core,
                idle_cycles: counter.read()?,
            })
        })
    }

    /// An open idle-accounting window. It cannot move to another executor or
    /// core: the window's length is the opening core's `mcycle`.
    pub struct Window {
        started_cycle: u64,
        start: [Option<Sample>; EXECUTORS],
        _local: PhantomData<*mut ()>,
    }

    impl Window {
        /// Open a window now. Call it from a task, never from an interrupt.
        pub fn begin() -> Self {
            let start = sample();
            Self {
                started_cycle: riscv::register::mcycle::read64(),
                start,
                _local: PhantomData,
            }
        }

        /// Close the window now.
        pub fn finish(self) -> Report {
            let ended_cycle = riscv::register::mcycle::read64();
            Report::between(self.started_cycle, self.start, ended_cycle, sample())
        }
    }
}

#[cfg(test)]
mod tests;
