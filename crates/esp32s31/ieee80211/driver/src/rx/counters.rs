//! Production counters of one Wi-Fi receive epoch, for the W1 log.
//!
//! The interrupt line (P3) and the radio executor (P2) each update their
//! own counters with single Relaxed atomic operations; a reader takes a
//! [`WifiRxReport`] at any time. Counts wrap; the reload maximum saturates
//! at the ring's attempt limit.

use core::sync::atomic::{AtomicU32, Ordering};

/// Counters beside the receive arena. Its zero bytes are zero counts.
#[derive(bytemuck::Zeroable, Default)]
pub struct WifiRxCounters {
    mac_irqs: AtomicU32,
    spurious_mac_irqs: AtomicU32,
    auxiliary_mac_irqs: AtomicU32,
    unhandled_mac_irqs: AtomicU32,
    power_irqs: AtomicU32,
    spurious_power_irqs: AtomicU32,
    doorbell_unmasks: AtomicU32,
    ignored_unmasks: AtomicU32,
}

/// One reading of [`WifiRxCounters`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WifiRxReport {
    /// MAC handler runs.
    pub mac_irqs: u32,
    /// MAC handler runs which saw no enabled cause.
    pub spurious_mac_irqs: u32,
    /// MAC handler runs which acknowledged an auxiliary cause.
    pub auxiliary_mac_irqs: u32,
    /// MAC handler runs which acknowledged a cause without a known meaning.
    pub unhandled_mac_irqs: u32,
    /// WDEVPWR handler runs which acknowledged a cause.
    pub power_irqs: u32,
    /// WDEVPWR handler runs which saw no enabled cause.
    pub spurious_power_irqs: u32,
    /// RX delivery restored through the doorbell.
    pub doorbell_unmasks: u32,
    /// Unmask requests ignored because both banks were masked.
    pub ignored_unmasks: u32,
}

fn bump(counter: &AtomicU32) {
    counter.fetch_add(1, Ordering::Relaxed);
}

impl WifiRxCounters {
    pub const fn new() -> Self {
        Self {
            mac_irqs: AtomicU32::new(0),
            spurious_mac_irqs: AtomicU32::new(0),
            auxiliary_mac_irqs: AtomicU32::new(0),
            unhandled_mac_irqs: AtomicU32::new(0),
            power_irqs: AtomicU32::new(0),
            spurious_power_irqs: AtomicU32::new(0),
            doorbell_unmasks: AtomicU32::new(0),
            ignored_unmasks: AtomicU32::new(0),
        }
    }

    /// Read every counter.
    pub fn report(&self) -> WifiRxReport {
        let load = |counter: &AtomicU32| counter.load(Ordering::Relaxed);
        WifiRxReport {
            mac_irqs: load(&self.mac_irqs),
            spurious_mac_irqs: load(&self.spurious_mac_irqs),
            auxiliary_mac_irqs: load(&self.auxiliary_mac_irqs),
            unhandled_mac_irqs: load(&self.unhandled_mac_irqs),
            power_irqs: load(&self.power_irqs),
            spurious_power_irqs: load(&self.spurious_power_irqs),
            doorbell_unmasks: load(&self.doorbell_unmasks),
            ignored_unmasks: load(&self.ignored_unmasks),
        }
    }

    #[inline(always)]
    pub(crate) fn mac_interrupt(&self, spurious: bool, auxiliary: bool, unhandled: bool) {
        bump(&self.mac_irqs);
        if spurious {
            bump(&self.spurious_mac_irqs);
        }
        if auxiliary {
            bump(&self.auxiliary_mac_irqs);
        }
        if unhandled {
            bump(&self.unhandled_mac_irqs);
        }
    }

    #[inline(always)]
    pub(crate) fn power_interrupt(&self, spurious: bool) {
        bump(if spurious {
            &self.spurious_power_irqs
        } else {
            &self.power_irqs
        });
    }

    #[inline(always)]
    pub(crate) fn doorbell_unmask(&self) {
        bump(&self.doorbell_unmasks);
    }

    #[inline(always)]
    pub(crate) fn ignored_unmask(&self) {
        bump(&self.ignored_unmasks);
    }
}
