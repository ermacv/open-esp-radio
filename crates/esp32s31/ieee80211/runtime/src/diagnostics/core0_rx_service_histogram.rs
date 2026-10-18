//! Diagnostic decomposition of Core0 RX service and local queue costs.
//!
//! This module is compiled only for the task-poll HIL image. Per-service bins
//! preserve fixed-frontier versus per-unit behavior without changing the
//! production service policy. Queue timings measure only the SPSC operation;
//! their counter-update self cost is reported separately.

use core::sync::atomic::{AtomicU32, Ordering};

use super::core0_rx_cycles::{Core0RxCycleSnapshot, cycle_count};

pub const CORE0_RX_SERVICE_HISTOGRAM_BINS: usize = 33;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Core0RxServiceBinSnapshot {
    pub services: u32,
    pub total: u32,
    pub setup: u32,
    pub frontier: u32,
    pub admission: u32,
    pub stage_take: u32,
    pub stage_pool: u32,
    pub publish: u32,
    pub tail: u32,
}

impl Core0RxServiceBinSnapshot {
    fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            services: self.services.wrapping_sub(earlier.services),
            total: self.total.wrapping_sub(earlier.total),
            setup: self.setup.wrapping_sub(earlier.setup),
            frontier: self.frontier.wrapping_sub(earlier.frontier),
            admission: self.admission.wrapping_sub(earlier.admission),
            stage_take: self.stage_take.wrapping_sub(earlier.stage_take),
            stage_pool: self.stage_pool.wrapping_sub(earlier.stage_pool),
            publish: self.publish.wrapping_sub(earlier.publish),
            tail: self.tail.wrapping_sub(earlier.tail),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Core0RxServiceHistogramSnapshot {
    pub bins: [Core0RxServiceBinSnapshot; CORE0_RX_SERVICE_HISTOGRAM_BINS],
    pub service_record_cycles: u32,
    pub spsc_push_calls: u32,
    pub spsc_push_full: u32,
    pub spsc_push_cycles: u32,
    pub spsc_pop_calls: u32,
    pub spsc_pop_empty: u32,
    pub spsc_pop_cycles: u32,
    pub spsc_record_cycles: u32,
}

impl Core0RxServiceHistogramSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        let mut bins = [Core0RxServiceBinSnapshot::default(); CORE0_RX_SERVICE_HISTOGRAM_BINS];
        for (index, bin) in bins.iter_mut().enumerate() {
            *bin = self.bins[index].wrapping_delta_since(earlier.bins[index]);
        }
        Self {
            bins,
            service_record_cycles: self
                .service_record_cycles
                .wrapping_sub(earlier.service_record_cycles),
            spsc_push_calls: self.spsc_push_calls.wrapping_sub(earlier.spsc_push_calls),
            spsc_push_full: self.spsc_push_full.wrapping_sub(earlier.spsc_push_full),
            spsc_push_cycles: self.spsc_push_cycles.wrapping_sub(earlier.spsc_push_cycles),
            spsc_pop_calls: self.spsc_pop_calls.wrapping_sub(earlier.spsc_pop_calls),
            spsc_pop_empty: self.spsc_pop_empty.wrapping_sub(earlier.spsc_pop_empty),
            spsc_pop_cycles: self.spsc_pop_cycles.wrapping_sub(earlier.spsc_pop_cycles),
            spsc_record_cycles: self
                .spsc_record_cycles
                .wrapping_sub(earlier.spsc_record_cycles),
        }
    }
}

impl Default for Core0RxServiceHistogramSnapshot {
    fn default() -> Self {
        Self {
            bins: [Core0RxServiceBinSnapshot::default(); CORE0_RX_SERVICE_HISTOGRAM_BINS],
            service_record_cycles: 0,
            spsc_push_calls: 0,
            spsc_push_full: 0,
            spsc_push_cycles: 0,
            spsc_pop_calls: 0,
            spsc_pop_empty: 0,
            spsc_pop_cycles: 0,
            spsc_record_cycles: 0,
        }
    }
}

struct Core0RxServiceBinCounters {
    services: AtomicU32,
    total: AtomicU32,
    setup: AtomicU32,
    frontier: AtomicU32,
    admission: AtomicU32,
    stage_take: AtomicU32,
    stage_pool: AtomicU32,
    publish: AtomicU32,
    tail: AtomicU32,
}

impl Core0RxServiceBinCounters {
    const fn new() -> Self {
        Self {
            services: AtomicU32::new(0),
            total: AtomicU32::new(0),
            setup: AtomicU32::new(0),
            frontier: AtomicU32::new(0),
            admission: AtomicU32::new(0),
            stage_take: AtomicU32::new(0),
            stage_pool: AtomicU32::new(0),
            publish: AtomicU32::new(0),
            tail: AtomicU32::new(0),
        }
    }

    fn record(&self, profile: &Core0RxCycleSnapshot) {
        self.services.fetch_add(1, Ordering::Relaxed);
        self.total.fetch_add(profile.total, Ordering::Relaxed);
        self.setup.fetch_add(profile.setup, Ordering::Relaxed);
        self.frontier.fetch_add(profile.frontier, Ordering::Relaxed);
        self.admission
            .fetch_add(profile.admission, Ordering::Relaxed);
        self.stage_take
            .fetch_add(profile.stage_take, Ordering::Relaxed);
        self.stage_pool
            .fetch_add(profile.stage_pool, Ordering::Relaxed);
        self.publish.fetch_add(profile.publish, Ordering::Relaxed);
        self.tail.fetch_add(profile.tail, Ordering::Relaxed);
    }

    fn snapshot(&self) -> Core0RxServiceBinSnapshot {
        Core0RxServiceBinSnapshot {
            services: self.services.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            setup: self.setup.load(Ordering::Relaxed),
            frontier: self.frontier.load(Ordering::Relaxed),
            admission: self.admission.load(Ordering::Relaxed),
            stage_take: self.stage_take.load(Ordering::Relaxed),
            stage_pool: self.stage_pool.load(Ordering::Relaxed),
            publish: self.publish.load(Ordering::Relaxed),
            tail: self.tail.load(Ordering::Relaxed),
        }
    }
}

pub struct Core0RxServiceHistogram {
    bins: [Core0RxServiceBinCounters; CORE0_RX_SERVICE_HISTOGRAM_BINS],
    service_record_cycles: AtomicU32,
    spsc_push_calls: AtomicU32,
    spsc_push_full: AtomicU32,
    spsc_push_cycles: AtomicU32,
    spsc_pop_calls: AtomicU32,
    spsc_pop_empty: AtomicU32,
    spsc_pop_cycles: AtomicU32,
    spsc_record_cycles: AtomicU32,
}

impl Core0RxServiceHistogram {
    pub const fn new() -> Self {
        Self {
            bins: [const { Core0RxServiceBinCounters::new() }; CORE0_RX_SERVICE_HISTOGRAM_BINS],
            service_record_cycles: AtomicU32::new(0),
            spsc_push_calls: AtomicU32::new(0),
            spsc_push_full: AtomicU32::new(0),
            spsc_push_cycles: AtomicU32::new(0),
            spsc_pop_calls: AtomicU32::new(0),
            spsc_pop_empty: AtomicU32::new(0),
            spsc_pop_cycles: AtomicU32::new(0),
            spsc_record_cycles: AtomicU32::new(0),
        }
    }

    pub(crate) fn record_service(&self, units: usize, profile: &Core0RxCycleSnapshot) {
        let started = cycle_count();
        self.bins[units.min(CORE0_RX_SERVICE_HISTOGRAM_BINS - 1)].record(profile);
        self.service_record_cycles
            .fetch_add(cycle_count().wrapping_sub(started), Ordering::Relaxed);
    }

    pub(crate) fn record_spsc_push(&self, cycles: u32, full: bool) {
        let started = cycle_count();
        self.spsc_push_calls.fetch_add(1, Ordering::Relaxed);
        self.spsc_push_cycles.fetch_add(cycles, Ordering::Relaxed);
        if full {
            self.spsc_push_full.fetch_add(1, Ordering::Relaxed);
        }
        self.spsc_record_cycles
            .fetch_add(cycle_count().wrapping_sub(started), Ordering::Relaxed);
    }

    pub(crate) fn record_spsc_pop(&self, cycles: u32, empty: bool) {
        let started = cycle_count();
        self.spsc_pop_calls.fetch_add(1, Ordering::Relaxed);
        self.spsc_pop_cycles.fetch_add(cycles, Ordering::Relaxed);
        if empty {
            self.spsc_pop_empty.fetch_add(1, Ordering::Relaxed);
        }
        self.spsc_record_cycles
            .fetch_add(cycle_count().wrapping_sub(started), Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Core0RxServiceHistogramSnapshot {
        let mut bins = [Core0RxServiceBinSnapshot::default(); CORE0_RX_SERVICE_HISTOGRAM_BINS];
        for (snapshot, counters) in bins.iter_mut().zip(&self.bins) {
            *snapshot = counters.snapshot();
        }
        Core0RxServiceHistogramSnapshot {
            bins,
            service_record_cycles: self.service_record_cycles.load(Ordering::Relaxed),
            spsc_push_calls: self.spsc_push_calls.load(Ordering::Relaxed),
            spsc_push_full: self.spsc_push_full.load(Ordering::Relaxed),
            spsc_push_cycles: self.spsc_push_cycles.load(Ordering::Relaxed),
            spsc_pop_calls: self.spsc_pop_calls.load(Ordering::Relaxed),
            spsc_pop_empty: self.spsc_pop_empty.load(Ordering::Relaxed),
            spsc_pop_cycles: self.spsc_pop_cycles.load(Ordering::Relaxed),
            spsc_record_cycles: self.spsc_record_cycles.load(Ordering::Relaxed),
        }
    }
}

impl Default for Core0RxServiceHistogram {
    fn default() -> Self {
        Self::new()
    }
}

pub static CORE0_RX_SERVICE_HISTOGRAM: Core0RxServiceHistogram = Core0RxServiceHistogram::new();

#[cfg(test)]
mod tests;
