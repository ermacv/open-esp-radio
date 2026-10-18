//! HIL classification of one bounded ESP32-S31 MAC interrupt drain.

use core::sync::atomic::{AtomicU32, Ordering};

use oer_esp32s31_ieee80211_mac::irq::{
    EVENT_COLLISION, EVENT_RX_SUCCESS, EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT,
};

const MAC_TX_IRQ_MASK: u32 = EVENT_TX_COMPLETE | EVENT_TX_TIMEOUT | EVENT_COLLISION;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacIrqClassificationSnapshot {
    pub spurious_entries: u32,
    pub rx_only_entries: u32,
    pub rx_mixed_entries: u32,
    pub tx_only_entries: u32,
    pub tx_mixed_entries: u32,
    pub other_only_entries: u32,
    pub extra_nonzero_snapshots: u32,
    pub saturated_entries: u32,
}

impl MacIrqClassificationSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            spurious_entries: self.spurious_entries.wrapping_sub(earlier.spurious_entries),
            rx_only_entries: self.rx_only_entries.wrapping_sub(earlier.rx_only_entries),
            rx_mixed_entries: self.rx_mixed_entries.wrapping_sub(earlier.rx_mixed_entries),
            tx_only_entries: self.tx_only_entries.wrapping_sub(earlier.tx_only_entries),
            tx_mixed_entries: self.tx_mixed_entries.wrapping_sub(earlier.tx_mixed_entries),
            other_only_entries: self
                .other_only_entries
                .wrapping_sub(earlier.other_only_entries),
            extra_nonzero_snapshots: self
                .extra_nonzero_snapshots
                .wrapping_sub(earlier.extra_nonzero_snapshots),
            saturated_entries: self
                .saturated_entries
                .wrapping_sub(earlier.saturated_entries),
        }
    }
}

pub struct MacIrqClassificationCounters {
    spurious_entries: AtomicU32,
    rx_only_entries: AtomicU32,
    rx_mixed_entries: AtomicU32,
    tx_only_entries: AtomicU32,
    tx_mixed_entries: AtomicU32,
    other_only_entries: AtomicU32,
    extra_nonzero_snapshots: AtomicU32,
    saturated_entries: AtomicU32,
    auxiliary_entries: AtomicU32,
    unhandled_entries: AtomicU32,
}

impl MacIrqClassificationCounters {
    pub const fn new() -> Self {
        Self {
            spurious_entries: AtomicU32::new(0),
            rx_only_entries: AtomicU32::new(0),
            rx_mixed_entries: AtomicU32::new(0),
            tx_only_entries: AtomicU32::new(0),
            tx_mixed_entries: AtomicU32::new(0),
            other_only_entries: AtomicU32::new(0),
            extra_nonzero_snapshots: AtomicU32::new(0),
            saturated_entries: AtomicU32::new(0),
            auxiliary_entries: AtomicU32::new(0),
            unhandled_entries: AtomicU32::new(0),
        }
    }

    #[inline]
    pub fn record(
        &self,
        had_status: bool,
        posted_events: u32,
        had_auxiliary_event: bool,
        had_unhandled_event: bool,
    ) {
        let rx = posted_events & EVENT_RX_SUCCESS != 0;
        let tx = posted_events & MAC_TX_IRQ_MASK != 0;
        let counter = if !had_status {
            &self.spurious_entries
        } else if rx && !tx && !had_unhandled_event {
            &self.rx_only_entries
        } else if rx {
            &self.rx_mixed_entries
        } else if tx && !had_auxiliary_event && !had_unhandled_event {
            &self.tx_only_entries
        } else if tx {
            &self.tx_mixed_entries
        } else {
            &self.other_only_entries
        };
        counter.fetch_add(1, Ordering::Relaxed);

        if had_auxiliary_event {
            self.auxiliary_entries.fetch_add(1, Ordering::Relaxed);
        }
        if had_unhandled_event {
            self.unhandled_entries.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn snapshot(&self) -> MacIrqClassificationSnapshot {
        MacIrqClassificationSnapshot {
            spurious_entries: self.spurious_entries.load(Ordering::Relaxed),
            rx_only_entries: self.rx_only_entries.load(Ordering::Relaxed),
            rx_mixed_entries: self.rx_mixed_entries.load(Ordering::Relaxed),
            tx_only_entries: self.tx_only_entries.load(Ordering::Relaxed),
            tx_mixed_entries: self.tx_mixed_entries.load(Ordering::Relaxed),
            other_only_entries: self.other_only_entries.load(Ordering::Relaxed),
            extra_nonzero_snapshots: self.extra_nonzero_snapshots.load(Ordering::Relaxed),
            saturated_entries: self.saturated_entries.load(Ordering::Relaxed),
        }
    }

    pub fn take_auxiliary_entries(&self) -> u32 {
        self.auxiliary_entries.swap(0, Ordering::Relaxed)
    }

    pub fn take_unhandled_entries(&self) -> u32 {
        self.unhandled_entries.swap(0, Ordering::Relaxed)
    }
}

impl Default for MacIrqClassificationCounters {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
