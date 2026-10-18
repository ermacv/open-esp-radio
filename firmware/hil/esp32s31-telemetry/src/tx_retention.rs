//! AP-epoch loss counters; traffic-window resets must not erase them.
use core::sync::atomic::{AtomicU32, Ordering};
use oer_esp32s31_ieee80211_runtime::diagnostics::aggregate_tx::NetworkTxRetentionDropReason;
use oer_hil_protocol::wifi::WifiTxRetentionEvidence;

pub struct TxRetentionCounters {
    active: AtomicU32,
    unicast: AtomicU32,
    group: AtomicU32,
}

impl TxRetentionCounters {
    pub const fn new() -> Self {
        Self {
            active: AtomicU32::new(0),
            unicast: AtomicU32::new(0),
            group: AtomicU32::new(0),
        }
    }

    pub fn observe(&self, reason: NetworkTxRetentionDropReason) {
        let counter = match reason {
            NetworkTxRetentionDropReason::ActiveQueueFull => &self.active,
            NetworkTxRetentionDropReason::UnicastPowerSaveFull => &self.unicast,
            NetworkTxRetentionDropReason::GroupPowerSaveFull => &self.group,
        };
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            Some(n.saturating_add(1))
        });
    }

    /// Called at an AP epoch boundary, while its producer is stopped.
    pub fn reset(&self) {
        self.active.store(0, Ordering::Relaxed);
        self.unicast.store(0, Ordering::Relaxed);
        self.group.store(0, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> WifiTxRetentionEvidence {
        WifiTxRetentionEvidence {
            active_queue_full: self.active.load(Ordering::Relaxed),
            unicast_power_save_full: self.unicast.load(Ordering::Relaxed),
            group_power_save_full: self.group.load(Ordering::Relaxed),
        }
    }
}

impl Default for TxRetentionCounters {
    fn default() -> Self {
        Self::new()
    }
}
