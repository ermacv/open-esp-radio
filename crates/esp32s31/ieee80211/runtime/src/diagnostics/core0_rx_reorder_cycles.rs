//! Core0 RX reorder-ingress phase accounting.
//!
//! This diagnostic profile ends before Ethernet dispatch. It decomposes the
//! previously opaque dequeue-to-dispatch interval without attributing all of
//! that time to either the scheduler or BlockAck logic by assumption.

pub(crate) use super::core0_paths::{Core0DirectPath, Core0ReorderPath};

use core::sync::atomic::{AtomicU32, Ordering};

use super::core0_rx_cycles::cycle_count;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Core0ReorderSnapshot {
    pub calls: u32,
    pub no_key: u32,
    pub inactive: u32,
    pub immediate: u32,
    pub slow: u32,
    pub total: u32,
    pub key: u32,
    pub bank: u32,
    pub ingress_observer: u32,
    pub first: u32,
    pub ingest: u32,
    pub deadline: u32,
    pub release_observer: u32,
    pub occupied_observer: u32,
    pub prepared_observer: u32,
    pub tail: u32,
    pub telemetry_record: u32,
    pub direct_calls: u32,
    pub direct_accepted: u32,
    pub direct_preflight_rejected: u32,
    pub direct_key_rejected: u32,
    pub direct_bank_rejected: u32,
    pub direct_reorder_rejected: u32,
    pub direct_duplicate_or_ignored: u32,
    pub direct_total: u32,
    pub direct_preflight: u32,
    pub direct_key: u32,
    pub direct_bank: u32,
    pub direct_ingest: u32,
    pub direct_deadline: u32,
    pub direct_dispatch: u32,
    pub direct_tail: u32,
    pub direct_telemetry_record: u32,
}

impl Core0ReorderSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            calls: self.calls.wrapping_sub(earlier.calls),
            no_key: self.no_key.wrapping_sub(earlier.no_key),
            inactive: self.inactive.wrapping_sub(earlier.inactive),
            immediate: self.immediate.wrapping_sub(earlier.immediate),
            slow: self.slow.wrapping_sub(earlier.slow),
            total: self.total.wrapping_sub(earlier.total),
            key: self.key.wrapping_sub(earlier.key),
            bank: self.bank.wrapping_sub(earlier.bank),
            ingress_observer: self.ingress_observer.wrapping_sub(earlier.ingress_observer),
            first: self.first.wrapping_sub(earlier.first),
            ingest: self.ingest.wrapping_sub(earlier.ingest),
            deadline: self.deadline.wrapping_sub(earlier.deadline),
            release_observer: self.release_observer.wrapping_sub(earlier.release_observer),
            occupied_observer: self
                .occupied_observer
                .wrapping_sub(earlier.occupied_observer),
            prepared_observer: self
                .prepared_observer
                .wrapping_sub(earlier.prepared_observer),
            tail: self.tail.wrapping_sub(earlier.tail),
            telemetry_record: self.telemetry_record.wrapping_sub(earlier.telemetry_record),
            direct_calls: self.direct_calls.wrapping_sub(earlier.direct_calls),
            direct_accepted: self.direct_accepted.wrapping_sub(earlier.direct_accepted),
            direct_preflight_rejected: self
                .direct_preflight_rejected
                .wrapping_sub(earlier.direct_preflight_rejected),
            direct_key_rejected: self
                .direct_key_rejected
                .wrapping_sub(earlier.direct_key_rejected),
            direct_bank_rejected: self
                .direct_bank_rejected
                .wrapping_sub(earlier.direct_bank_rejected),
            direct_reorder_rejected: self
                .direct_reorder_rejected
                .wrapping_sub(earlier.direct_reorder_rejected),
            direct_duplicate_or_ignored: self
                .direct_duplicate_or_ignored
                .wrapping_sub(earlier.direct_duplicate_or_ignored),
            direct_total: self.direct_total.wrapping_sub(earlier.direct_total),
            direct_preflight: self.direct_preflight.wrapping_sub(earlier.direct_preflight),
            direct_key: self.direct_key.wrapping_sub(earlier.direct_key),
            direct_bank: self.direct_bank.wrapping_sub(earlier.direct_bank),
            direct_ingest: self.direct_ingest.wrapping_sub(earlier.direct_ingest),
            direct_deadline: self.direct_deadline.wrapping_sub(earlier.direct_deadline),
            direct_dispatch: self.direct_dispatch.wrapping_sub(earlier.direct_dispatch),
            direct_tail: self.direct_tail.wrapping_sub(earlier.direct_tail),
            direct_telemetry_record: self
                .direct_telemetry_record
                .wrapping_sub(earlier.direct_telemetry_record),
        }
    }
}

pub struct Core0ReorderCounters {
    calls: AtomicU32,
    no_key: AtomicU32,
    inactive: AtomicU32,
    immediate: AtomicU32,
    slow: AtomicU32,
    total: AtomicU32,
    key: AtomicU32,
    bank: AtomicU32,
    ingress_observer: AtomicU32,
    first: AtomicU32,
    ingest: AtomicU32,
    deadline: AtomicU32,
    release_observer: AtomicU32,
    occupied_observer: AtomicU32,
    prepared_observer: AtomicU32,
    tail: AtomicU32,
    telemetry_record: AtomicU32,
    active_last: AtomicU32,
    direct_calls: AtomicU32,
    direct_accepted: AtomicU32,
    direct_preflight_rejected: AtomicU32,
    direct_key_rejected: AtomicU32,
    direct_bank_rejected: AtomicU32,
    direct_reorder_rejected: AtomicU32,
    direct_duplicate_or_ignored: AtomicU32,
    direct_total: AtomicU32,
    direct_preflight: AtomicU32,
    direct_key: AtomicU32,
    direct_bank: AtomicU32,
    direct_ingest: AtomicU32,
    direct_deadline: AtomicU32,
    direct_dispatch: AtomicU32,
    direct_tail: AtomicU32,
    direct_telemetry_record: AtomicU32,
}

impl Core0ReorderCounters {
    pub const fn new() -> Self {
        Self {
            calls: AtomicU32::new(0),
            no_key: AtomicU32::new(0),
            inactive: AtomicU32::new(0),
            immediate: AtomicU32::new(0),
            slow: AtomicU32::new(0),
            total: AtomicU32::new(0),
            key: AtomicU32::new(0),
            bank: AtomicU32::new(0),
            ingress_observer: AtomicU32::new(0),
            first: AtomicU32::new(0),
            ingest: AtomicU32::new(0),
            deadline: AtomicU32::new(0),
            release_observer: AtomicU32::new(0),
            occupied_observer: AtomicU32::new(0),
            prepared_observer: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            telemetry_record: AtomicU32::new(0),
            active_last: AtomicU32::new(0),
            direct_calls: AtomicU32::new(0),
            direct_accepted: AtomicU32::new(0),
            direct_preflight_rejected: AtomicU32::new(0),
            direct_key_rejected: AtomicU32::new(0),
            direct_bank_rejected: AtomicU32::new(0),
            direct_reorder_rejected: AtomicU32::new(0),
            direct_duplicate_or_ignored: AtomicU32::new(0),
            direct_total: AtomicU32::new(0),
            direct_preflight: AtomicU32::new(0),
            direct_key: AtomicU32::new(0),
            direct_bank: AtomicU32::new(0),
            direct_ingest: AtomicU32::new(0),
            direct_deadline: AtomicU32::new(0),
            direct_dispatch: AtomicU32::new(0),
            direct_tail: AtomicU32::new(0),
            direct_telemetry_record: AtomicU32::new(0),
        }
    }

    #[inline(always)]
    fn record_phase(&self, phase: &AtomicU32) {
        let phase_ended = cycle_count();
        let elapsed = phase_ended.wrapping_sub(self.active_last.load(Ordering::Relaxed));
        let telemetry_started = cycle_count();
        phase.fetch_add(elapsed, Ordering::Relaxed);
        self.total.fetch_add(elapsed, Ordering::Relaxed);
        let telemetry_ended = cycle_count();
        self.telemetry_record.fetch_add(
            telemetry_ended.wrapping_sub(telemetry_started),
            Ordering::Relaxed,
        );
        self.active_last.store(telemetry_ended, Ordering::Relaxed);
    }

    #[inline(always)]
    fn record_path(&self, path: Core0ReorderPath) {
        let telemetry_started = cycle_count();
        self.calls.fetch_add(1, Ordering::Relaxed);
        match path {
            Core0ReorderPath::NoKey => self.no_key.fetch_add(1, Ordering::Relaxed),
            Core0ReorderPath::Inactive => self.inactive.fetch_add(1, Ordering::Relaxed),
            Core0ReorderPath::Immediate => self.immediate.fetch_add(1, Ordering::Relaxed),
            Core0ReorderPath::Slow => self.slow.fetch_add(1, Ordering::Relaxed),
        };
        self.telemetry_record.fetch_add(
            cycle_count().wrapping_sub(telemetry_started),
            Ordering::Relaxed,
        );
    }

    /// Begin one synchronous reorder-ingress transaction.
    ///
    /// The connected protocol has one Core0 execution owner, so this active
    /// timestamp cannot be nested or observed by another caller.
    #[inline(always)]
    pub(crate) fn begin(&self) {
        self.active_last.store(cycle_count(), Ordering::Relaxed);
    }

    #[inline(always)]
    pub(crate) fn key_completed(&self) {
        self.record_phase(&self.key);
    }

    #[inline(always)]
    pub(crate) fn bank_completed(&self) {
        self.record_phase(&self.bank);
    }

    #[inline(always)]
    pub(crate) fn ingress_observer_completed(&self) {
        self.record_phase(&self.ingress_observer);
    }

    #[inline(always)]
    pub(crate) fn first_completed(&self) {
        self.record_phase(&self.first);
    }

    #[inline(always)]
    pub(crate) fn ingest_completed(&self) {
        self.record_phase(&self.ingest);
    }

    #[inline(always)]
    pub(crate) fn deadline_completed(&self) {
        self.record_phase(&self.deadline);
    }

    #[inline(always)]
    pub(crate) fn release_observer_completed(&self) {
        self.record_phase(&self.release_observer);
    }

    #[inline(always)]
    pub(crate) fn occupied_observer_completed(&self) {
        self.record_phase(&self.occupied_observer);
    }

    #[inline(always)]
    pub(crate) fn prepared_observer_completed(&self) {
        self.record_phase(&self.prepared_observer);
    }

    #[inline(always)]
    pub(crate) fn finish(&self, path: Core0ReorderPath) {
        self.record_phase(&self.tail);
        self.record_path(path);
    }

    pub fn snapshot(&self) -> Core0ReorderSnapshot {
        Core0ReorderSnapshot {
            calls: self.calls.load(Ordering::Relaxed),
            no_key: self.no_key.load(Ordering::Relaxed),
            inactive: self.inactive.load(Ordering::Relaxed),
            immediate: self.immediate.load(Ordering::Relaxed),
            slow: self.slow.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            key: self.key.load(Ordering::Relaxed),
            bank: self.bank.load(Ordering::Relaxed),
            ingress_observer: self.ingress_observer.load(Ordering::Relaxed),
            first: self.first.load(Ordering::Relaxed),
            ingest: self.ingest.load(Ordering::Relaxed),
            deadline: self.deadline.load(Ordering::Relaxed),
            release_observer: self.release_observer.load(Ordering::Relaxed),
            occupied_observer: self.occupied_observer.load(Ordering::Relaxed),
            prepared_observer: self.prepared_observer.load(Ordering::Relaxed),
            tail: self.tail.load(Ordering::Relaxed),
            telemetry_record: self.telemetry_record.load(Ordering::Relaxed),
            direct_calls: self.direct_calls.load(Ordering::Relaxed),
            direct_accepted: self.direct_accepted.load(Ordering::Relaxed),
            direct_preflight_rejected: self.direct_preflight_rejected.load(Ordering::Relaxed),
            direct_key_rejected: self.direct_key_rejected.load(Ordering::Relaxed),
            direct_bank_rejected: self.direct_bank_rejected.load(Ordering::Relaxed),
            direct_reorder_rejected: self.direct_reorder_rejected.load(Ordering::Relaxed),
            direct_duplicate_or_ignored: self.direct_duplicate_or_ignored.load(Ordering::Relaxed),
            direct_total: self.direct_total.load(Ordering::Relaxed),
            direct_preflight: self.direct_preflight.load(Ordering::Relaxed),
            direct_key: self.direct_key.load(Ordering::Relaxed),
            direct_bank: self.direct_bank.load(Ordering::Relaxed),
            direct_ingest: self.direct_ingest.load(Ordering::Relaxed),
            direct_deadline: self.direct_deadline.load(Ordering::Relaxed),
            direct_dispatch: self.direct_dispatch.load(Ordering::Relaxed),
            direct_tail: self.direct_tail.load(Ordering::Relaxed),
            direct_telemetry_record: self.direct_telemetry_record.load(Ordering::Relaxed),
        }
    }

    #[inline(always)]
    fn record_direct(&self, profile: Core0DirectCycleProfile, ended: u32) {
        let telemetry_started = cycle_count();
        self.direct_calls.fetch_add(1, Ordering::Relaxed);
        match profile.path {
            Core0DirectPath::Accepted => self.direct_accepted.fetch_add(1, Ordering::Relaxed),
            Core0DirectPath::PreflightRejected => self
                .direct_preflight_rejected
                .fetch_add(1, Ordering::Relaxed),
            Core0DirectPath::KeyRejected => {
                self.direct_key_rejected.fetch_add(1, Ordering::Relaxed)
            }
            Core0DirectPath::BankRejected => {
                self.direct_bank_rejected.fetch_add(1, Ordering::Relaxed)
            }
            Core0DirectPath::ReorderRejected => {
                self.direct_reorder_rejected.fetch_add(1, Ordering::Relaxed)
            }
            Core0DirectPath::DuplicateOrIgnored => self
                .direct_duplicate_or_ignored
                .fetch_add(1, Ordering::Relaxed),
        };
        self.direct_total
            .fetch_add(ended.wrapping_sub(profile.started), Ordering::Relaxed);
        self.direct_preflight
            .fetch_add(profile.preflight, Ordering::Relaxed);
        self.direct_key.fetch_add(profile.key, Ordering::Relaxed);
        self.direct_bank.fetch_add(profile.bank, Ordering::Relaxed);
        self.direct_ingest
            .fetch_add(profile.ingest, Ordering::Relaxed);
        self.direct_deadline
            .fetch_add(profile.deadline, Ordering::Relaxed);
        self.direct_dispatch
            .fetch_add(profile.dispatch, Ordering::Relaxed);
        self.direct_tail.fetch_add(profile.tail, Ordering::Relaxed);
        self.direct_telemetry_record.fetch_add(
            cycle_count().wrapping_sub(telemetry_started),
            Ordering::Relaxed,
        );
    }
}

impl Default for Core0ReorderCounters {
    fn default() -> Self {
        Self::new()
    }
}

pub static CORE0_REORDER_CYCLES: Core0ReorderCounters = Core0ReorderCounters::new();

pub(crate) struct Core0DirectCycleProfile {
    started: u32,
    last: u32,
    preflight: u32,
    key: u32,
    bank: u32,
    ingest: u32,
    deadline: u32,
    dispatch: u32,
    tail: u32,
    path: Core0DirectPath,
}

impl Core0DirectCycleProfile {
    #[inline(always)]
    pub(crate) fn begin() -> Self {
        let now = cycle_count();
        Self {
            started: now,
            last: now,
            preflight: 0,
            key: 0,
            bank: 0,
            ingest: 0,
            deadline: 0,
            dispatch: 0,
            tail: 0,
            path: Core0DirectPath::PreflightRejected,
        }
    }

    #[inline(always)]
    fn complete_phase(last: &mut u32) -> u32 {
        let now = cycle_count();
        let elapsed = now.wrapping_sub(*last);
        *last = now;
        elapsed
    }

    #[inline(always)]
    pub(crate) fn preflight_completed(&mut self) {
        self.preflight = Self::complete_phase(&mut self.last);
    }

    #[inline(always)]
    pub(crate) fn key_completed(&mut self) {
        self.key = Self::complete_phase(&mut self.last);
    }

    #[inline(always)]
    pub(crate) fn bank_completed(&mut self) {
        self.bank = Self::complete_phase(&mut self.last);
    }

    #[inline(always)]
    pub(crate) fn ingest_completed(&mut self) {
        self.ingest = Self::complete_phase(&mut self.last);
    }

    #[inline(always)]
    pub(crate) fn deadline_completed(&mut self) {
        self.deadline = Self::complete_phase(&mut self.last);
    }

    #[inline(always)]
    pub(crate) fn dispatch_completed(&mut self) {
        self.dispatch = Self::complete_phase(&mut self.last);
    }

    #[inline(always)]
    pub(crate) fn finish(mut self, path: Core0DirectPath) {
        let ended = cycle_count();
        self.tail = ended.wrapping_sub(self.last);
        self.path = path;
        CORE0_REORDER_CYCLES.record_direct(self, ended);
    }
}

#[cfg(test)]
mod tests;
