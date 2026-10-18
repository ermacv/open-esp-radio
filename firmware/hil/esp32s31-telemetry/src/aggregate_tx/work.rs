//! Separate ordinary and aggregate work at terminal edges.

use core::sync::atomic::{AtomicU32, Ordering};
use oer_esp32s31_ieee80211_runtime::diagnostics::aggregate_tx::MacTxWork;

pub(super) struct WorkCounters {
    exchanges: AtomicU32,
    publications: AtomicU32,
    psdu_bytes: AtomicU32,
    mpdus: AtomicU32,
    nominal_data_micros: AtomicU32,
    unestimated_publications: AtomicU32,
    ppdu_micros: AtomicU32,
    unestimated_ppdus: AtomicU32,
    aifs_slots: AtomicU32,
    backoff_slots: AtomicU32,
    unreported_contention: AtomicU32,
    saturated_exchanges: AtomicU32,
}

impl WorkCounters {
    pub(super) const fn new() -> Self {
        Self {
            exchanges: AtomicU32::new(0),
            publications: AtomicU32::new(0),
            psdu_bytes: AtomicU32::new(0),
            mpdus: AtomicU32::new(0),
            nominal_data_micros: AtomicU32::new(0),
            unestimated_publications: AtomicU32::new(0),
            ppdu_micros: AtomicU32::new(0),
            unestimated_ppdus: AtomicU32::new(0),
            aifs_slots: AtomicU32::new(0),
            backoff_slots: AtomicU32::new(0),
            unreported_contention: AtomicU32::new(0),
            saturated_exchanges: AtomicU32::new(0),
        }
    }

    pub(super) fn record(&self, work: MacTxWork) {
        self.ppdu_micros
            .fetch_add(work.ppdu_micros, Ordering::Relaxed);
        self.unestimated_ppdus
            .fetch_add(work.unestimated_ppdus, Ordering::Relaxed);
        self.aifs_slots
            .fetch_add(work.aifs_slots, Ordering::Relaxed);
        self.backoff_slots
            .fetch_add(work.backoff_slots, Ordering::Relaxed);
        self.unreported_contention
            .fetch_add(work.unreported_contention, Ordering::Relaxed);
        self.exchanges.fetch_add(1, Ordering::Relaxed);
        self.publications
            .fetch_add(work.publications, Ordering::Relaxed);
        self.psdu_bytes
            .fetch_add(work.psdu_bytes, Ordering::Relaxed);
        self.mpdus.fetch_add(work.mpdus, Ordering::Relaxed);
        self.nominal_data_micros
            .fetch_add(work.nominal_data_micros, Ordering::Relaxed);
        self.unestimated_publications
            .fetch_add(work.unestimated_publications, Ordering::Relaxed);
        self.saturated_exchanges
            .fetch_add(u32::from(work.saturated), Ordering::Relaxed);
    }

    pub(super) fn snapshot(&self) -> AggregateTxWorkSnapshot {
        AggregateTxWorkSnapshot {
            exchanges: self.exchanges.load(Ordering::Relaxed),
            publications: self.publications.load(Ordering::Relaxed),
            psdu_bytes: self.psdu_bytes.load(Ordering::Relaxed),
            mpdus: self.mpdus.load(Ordering::Relaxed),
            nominal_data_micros: self.nominal_data_micros.load(Ordering::Relaxed),
            unestimated_publications: self.unestimated_publications.load(Ordering::Relaxed),
            ppdu_micros: self.ppdu_micros.load(Ordering::Relaxed),
            unestimated_ppdus: self.unestimated_ppdus.load(Ordering::Relaxed),
            aifs_slots: self.aifs_slots.load(Ordering::Relaxed),
            backoff_slots: self.backoff_slots.load(Ordering::Relaxed),
            unreported_contention: self.unreported_contention.load(Ordering::Relaxed),
            saturated_exchanges: self.saturated_exchanges.load(Ordering::Relaxed),
        }
    }
}

/// Counters wrap at u32; deltas are valid within one wrap.
/// Each bank includes terminal exchanges of its own class. Fallback is ordinary.
/// Nominal data and PPDU times are estimates, not measured airtime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AggregateTxWorkSnapshot {
    pub exchanges: u32,
    pub publications: u32,
    pub psdu_bytes: u32,
    pub mpdus: u32,
    pub nominal_data_micros: u32,
    pub unestimated_publications: u32,
    pub ppdu_micros: u32,
    pub unestimated_ppdus: u32,
    pub aifs_slots: u32,
    pub backoff_slots: u32,
    pub unreported_contention: u32,
    pub saturated_exchanges: u32,
}

impl AggregateTxWorkSnapshot {
    pub(super) fn delta_since(self, earlier: Self) -> Self {
        Self {
            exchanges: self.exchanges.wrapping_sub(earlier.exchanges),
            publications: self.publications.wrapping_sub(earlier.publications),
            psdu_bytes: self.psdu_bytes.wrapping_sub(earlier.psdu_bytes),
            mpdus: self.mpdus.wrapping_sub(earlier.mpdus),
            nominal_data_micros: self
                .nominal_data_micros
                .wrapping_sub(earlier.nominal_data_micros),
            unestimated_publications: self
                .unestimated_publications
                .wrapping_sub(earlier.unestimated_publications),
            ppdu_micros: self.ppdu_micros.wrapping_sub(earlier.ppdu_micros),
            unestimated_ppdus: self
                .unestimated_ppdus
                .wrapping_sub(earlier.unestimated_ppdus),
            aifs_slots: self.aifs_slots.wrapping_sub(earlier.aifs_slots),
            backoff_slots: self.backoff_slots.wrapping_sub(earlier.backoff_slots),
            unreported_contention: self
                .unreported_contention
                .wrapping_sub(earlier.unreported_contention),
            saturated_exchanges: self
                .saturated_exchanges
                .wrapping_sub(earlier.saturated_exchanges),
        }
    }
}
