//! The duration of each poll of a future, or of each run of an interrupt
//! handler, in a histogram of cycles. No lock and no critical section is
//! taken on any path, so an interrupt handler records as cheaply as a task.
//!
//! A [`CycleHistogram`] has one writer: the one task or interrupt handler
//! that records into it. A record is a load and a store of the sample's
//! bucket and of the maximum, never a read-modify-write that another writer
//! could interleave, so a second writer that preempts the first can lose a
//! sample. Readers anywhere take a [`CycleSnapshot`]; each counter in it is
//! exact, and the snapshot as a whole is exact once the writer has stopped
//! (a report printed after its run): a snapshot taken during a run may miss
//! the samples recorded while it was being copied.
//!
//! The buckets are log-linear: below 16 cycles one per value, above it 8 per
//! power of two, so a bucket's width is at most an eighth of its lower bound.
//! A percentile is therefore an upper bound that exceeds the true value by
//! less than an eighth of it, the maximum is exact, and a count over a
//! threshold is exact when the threshold ends a bucket and otherwise a range
//! ([`OverCount`]).
//!
//! [`Timed`] wraps a future and records each of its polls. The cycles come
//! from a [`CycleSource`]: on the chip [`Mcycle`], the low 32 bits of the
//! core's `mcycle`, which is correct for polls shorter than 2³² cycles (over
//! ten seconds at 400 MHz); a host test supplies a fake counter.

use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::{Context, Poll};

/// Bits of a value below its leading one that select its bucket.
const SUB_BITS: u32 = 3;
/// Buckets per power of two.
const SUB_BUCKETS: u32 = 1 << SUB_BITS;
/// Buckets of a histogram over `u32` cycles: the exact values below
/// `2 · SUB_BUCKETS`, and `SUB_BUCKETS` for each power of two above.
pub const BUCKETS: usize = ((u32::BITS - SUB_BITS + 1) * SUB_BUCKETS) as usize;

/// The bucket of `cycles`.
const fn bucket(cycles: u32) -> usize {
    if cycles < SUB_BUCKETS {
        return cycles as usize;
    }
    let shift = u32::BITS - 1 - cycles.leading_zeros() - SUB_BITS;
    (shift * SUB_BUCKETS + (cycles >> shift)) as usize
}

/// The smallest and the largest value of bucket `index`.
const fn bucket_bounds(index: usize) -> (u32, u32) {
    let index = index as u32;
    if index < SUB_BUCKETS {
        return (index, index);
    }
    let shift = index / SUB_BUCKETS - 1;
    let low = (index % SUB_BUCKETS + SUB_BUCKETS) << shift;
    (low, low + ((1 << shift) - 1))
}

/// A histogram of cycle counts with one writer (module documentation).
pub struct CycleHistogram {
    buckets: [AtomicU32; BUCKETS],
    max: AtomicU32,
}

impl CycleHistogram {
    /// An empty histogram, for a `static`.
    pub const fn new() -> Self {
        Self {
            buckets: [const { AtomicU32::new(0) }; BUCKETS],
            max: AtomicU32::new(0),
        }
    }

    /// Record one sample of `cycles`. Only this histogram's one writer calls
    /// it. A bucket that has counted `u32::MAX` samples stays there.
    #[inline]
    pub fn record(&self, cycles: u32) {
        // Every value has a bucket.
        if let Some(bucket) = self.buckets.get(bucket(cycles)) {
            bucket.store(
                bucket.load(Ordering::Relaxed).saturating_add(1),
                Ordering::Relaxed,
            );
        }
        if cycles > self.max.load(Ordering::Relaxed) {
            self.max.store(cycles, Ordering::Relaxed);
        }
    }

    /// The counts now.
    pub fn snapshot(&self) -> CycleSnapshot {
        CycleSnapshot {
            max: self.max.load(Ordering::Relaxed),
            buckets: self
                .buckets
                .each_ref()
                .map(|bucket| bucket.load(Ordering::Relaxed)),
        }
    }
}

impl Default for CycleHistogram {
    fn default() -> Self {
        Self::new()
    }
}

/// The counts of a [`CycleHistogram`] at one instant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CycleSnapshot {
    buckets: [u32; BUCKETS],
    max: u32,
}

/// How many samples exceed a threshold: exact when both bounds agree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OverCount {
    /// Samples in buckets whose every value exceeds the threshold.
    pub at_least: u64,
    /// `at_least` and the samples of the bucket that holds the threshold
    /// and values above it.
    pub at_most: u64,
}

impl OverCount {
    /// The count, when the threshold ends a bucket or no sample shares the
    /// threshold's bucket.
    pub fn exact(self) -> Option<u64> {
        (self.at_least == self.at_most).then_some(self.at_least)
    }
}

impl CycleSnapshot {
    /// The number of samples.
    pub fn count(&self) -> u64 {
        self.buckets.iter().map(|&count| u64::from(count)).sum()
    }

    /// The longest sample, exact; `None` without samples.
    pub fn max(&self) -> Option<u32> {
        (self.count() > 0).then_some(self.max)
    }

    /// A duration that at least `percent` % of the samples do not exceed:
    /// the largest value of the bucket that holds the sample of that rank,
    /// at most the maximum. `None` without samples or above 100 %.
    pub fn percentile(&self, percent: u8) -> Option<u32> {
        let count = self.count();
        if count == 0 || percent > 100 {
            return None;
        }
        let rank = (count * u64::from(percent)).div_ceil(100).max(1);
        let mut seen = 0;
        for (index, &samples) in self.buckets.iter().enumerate() {
            seen += u64::from(samples);
            if seen >= rank {
                return Some(bucket_bounds(index).1.min(self.max));
            }
        }
        // The ranks end at `count`, the sum of every bucket.
        None
    }

    /// The samples longer than `threshold_cycles`.
    pub fn count_over(&self, threshold_cycles: u32) -> OverCount {
        let threshold_bucket = bucket(threshold_cycles);
        let above: u64 = self
            .buckets
            .iter()
            .skip(threshold_bucket + 1)
            .map(|&count| u64::from(count))
            .sum();
        let shared = match self.buckets.get(threshold_bucket) {
            Some(&count)
                if bucket_bounds(threshold_bucket).1 > threshold_cycles
                    && self.max > threshold_cycles =>
            {
                u64::from(count)
            }
            _ => 0,
        };
        OverCount {
            at_least: above,
            at_most: above + shared,
        }
    }
}

/// A free-running cycle counter, read as its low 32 bits.
pub trait CycleSource {
    /// The counter now; a duration is the wrapping difference of two reads.
    fn now(&self) -> u32;
}

impl<C: CycleSource + ?Sized> CycleSource for &C {
    fn now(&self) -> u32 {
        (**self).now()
    }
}

/// The low 32 bits of the calling core's `mcycle`.
#[cfg(feature = "chip")]
#[derive(Clone, Copy, Debug, Default)]
pub struct Mcycle;

#[cfg(feature = "chip")]
impl CycleSource for Mcycle {
    #[inline(always)]
    fn now(&self) -> u32 {
        oer_riscv::cycle32()
    }
}

pin_project_lite::pin_project! {
    /// The future `F`, each poll's cycles from `C` recorded in a histogram.
    pub struct Timed<F, C> {
        #[pin]
        future: F,
        histogram: &'static CycleHistogram,
        #[pin]
        source: C,
    }
}

impl<F: Future, C: CycleSource> Timed<F, C> {
    /// Record each poll of `future` into `histogram`, of which this future
    /// is the one writer.
    pub fn new(future: F, histogram: &'static CycleHistogram, source: C) -> Self {
        Self {
            future,
            histogram,
            source,
        }
    }
}

impl<F: Future, C: CycleSource> Future for Timed<F, C> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.project();
        let started = this.source.now();
        let poll = this.future.poll(context);
        this.histogram
            .record(this.source.now().wrapping_sub(started));
        poll
    }
}

#[cfg(test)]
mod tests;
