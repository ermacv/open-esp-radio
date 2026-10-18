//! Terminal receipts only; no intermediate BA or live-tail inference.
use core::sync::atomic::{AtomicU32, Ordering};
use oer_hil_protocol::network::StationTxTerminalEvidence;
use oer_ieee80211_softmac::{MacAmpduTxResult, MacAmpduTxStatus};

pub(super) struct Counters {
    exchanges: AtomicU32,
    mpdus: AtomicU32,
    acknowledged: AtomicU32,
    unacknowledged: AtomicU32,
    ordinary_recovered: AtomicU32,
    ordinary_failed: AtomicU32,
    invalid_statuses: AtomicU32,
}

impl Counters {
    pub(super) const fn new() -> Self {
        Self {
            exchanges: AtomicU32::new(0),
            mpdus: AtomicU32::new(0),
            acknowledged: AtomicU32::new(0),
            unacknowledged: AtomicU32::new(0),
            ordinary_recovered: AtomicU32::new(0),
            ordinary_failed: AtomicU32::new(0),
            invalid_statuses: AtomicU32::new(0),
        }
    }

    pub(super) fn record<R: Copy>(&self, status: MacAmpduTxStatus<R>) {
        let acknowledged = status.delivered_subframes();
        let Some(unacknowledged) = status.original_subframes.checked_sub(acknowledged) else {
            self.invalid_statuses.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if status.original_subframes == 0
            || (matches!(status.result, MacAmpduTxResult::Delivered) != (unacknowledged == 0))
        {
            self.invalid_statuses.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.exchanges.fetch_add(1, Ordering::Relaxed);
        self.mpdus
            .fetch_add(u32::from(status.original_subframes), Ordering::Relaxed);
        self.acknowledged
            .fetch_add(u32::from(acknowledged), Ordering::Relaxed);
        self.unacknowledged
            .fetch_add(u32::from(unacknowledged), Ordering::Relaxed);
        let individual = status.individual_retries;
        self.ordinary_recovered
            .fetch_add(u32::from(individual.transmitted), Ordering::Relaxed);
        self.ordinary_failed
            .fetch_add(u32::from(individual.failed), Ordering::Relaxed);
    }

    pub(super) fn snapshot(&self) -> StationTxTerminalEvidence {
        StationTxTerminalEvidence {
            exchanges: self.exchanges.load(Ordering::Relaxed),
            mpdus: self.mpdus.load(Ordering::Relaxed),
            acknowledged: self.acknowledged.load(Ordering::Relaxed),
            unacknowledged: self.unacknowledged.load(Ordering::Relaxed),
            ordinary_recovered: self.ordinary_recovered.load(Ordering::Relaxed),
            ordinary_failed: self.ordinary_failed.load(Ordering::Relaxed),
            invalid_statuses: self.invalid_statuses.load(Ordering::Relaxed),
        }
    }
}

pub(super) fn delta(
    now: StationTxTerminalEvidence,
    before: StationTxTerminalEvidence,
) -> StationTxTerminalEvidence {
    StationTxTerminalEvidence {
        exchanges: now.exchanges.wrapping_sub(before.exchanges),
        mpdus: now.mpdus.wrapping_sub(before.mpdus),
        acknowledged: now.acknowledged.wrapping_sub(before.acknowledged),
        unacknowledged: now.unacknowledged.wrapping_sub(before.unacknowledged),
        ordinary_recovered: now
            .ordinary_recovered
            .wrapping_sub(before.ordinary_recovered),
        ordinary_failed: now.ordinary_failed.wrapping_sub(before.ordinary_failed),
        invalid_statuses: now.invalid_statuses.wrapping_sub(before.invalid_statuses),
    }
}

#[cfg(test)]
mod tests;
