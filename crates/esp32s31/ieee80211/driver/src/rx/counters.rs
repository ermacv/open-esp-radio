//! Production counters of one Wi-Fi receive epoch, for the W1 log.
//!
//! The interrupt line (P3), the radio executor (P2) and the application's
//! frame receiver (P1, the prefix counts) each update their own counters
//! with single Relaxed atomic operations; a reader takes a [`WifiRxReport`]
//! at any time. Counts wrap; the reload maximum is the
//! ring's own evidence and stops at its attempt limit.

use core::sync::atomic::{AtomicU32, Ordering};

use oer_esp32s31_ieee80211_mac::rx::RxError;

use super::transaction::Discard;

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
    units: AtomicU32,
    bytes: AtomicU32,
    chained_drops: AtomicU32,
    oversized_drops: AtomicU32,
    empty_drops: AtomicU32,
    too_long_drops: AtomicU32,
    overload_drops: AtomicU32,
    admission_blocks: AtomicU32,
    stage_capacity_blocks: AtomicU32,
    service_passes: AtomicU32,
    service_errors: AtomicU32,
    immediate_continuations: AtomicU32,
    timed_continuations: AtomicU32,
    max_reload_attempts: AtomicU32,
    prefix: PrefixCounters,
}

/// The application's counts of the RX-control prefix decode.
#[derive(bytemuck::Zeroable, Default)]
struct PrefixCounters {
    invalid: AtomicU32,
    chain: AtomicU32,
    bounds: AtomicU32,
    rx_failure: AtomicU32,
    mic_failure: AtomicU32,
    quarantined: AtomicU32,
    output_small: AtomicU32,
    metadata: AtomicU32,
    ignored: AtomicU32,
    unsupported: AtomicU32,
    strict_rxend_refusable: AtomicU32,
    strict_dump_refusable: AtomicU32,
}

/// One reading of the prefix decode counts: refusals by their
/// [`RxError`], and admitted frames a strict check would have refused.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WifiRxPrefixReport {
    pub invalid: u32,
    pub chain: u32,
    pub bounds: u32,
    /// A nonzero RX state other than the two below.
    pub rx_failure: u32,
    /// RX state `0xf5`.
    pub mic_failure: u32,
    /// RX state `0xc6`.
    pub quarantined: u32,
    pub output_small: u32,
    pub metadata: u32,
    pub ignored: u32,
    pub unsupported: u32,
    /// Admitted frames with a nonzero RXEND state.
    pub strict_rxend_refusable: u32,
    /// Admitted frames whose dump length is not `sig_len` plus the FCS.
    pub strict_dump_refusable: u32,
}

impl WifiRxPrefixReport {
    /// The refusals counted for `reason`.
    pub const fn refused(&self, reason: RxError) -> u32 {
        match reason {
            RxError::Invalid => self.invalid,
            RxError::Chain => self.chain,
            RxError::Bounds => self.bounds,
            RxError::RxFailure => self.rx_failure,
            RxError::MicFailure => self.mic_failure,
            RxError::Quarantined => self.quarantined,
            RxError::OutputSmall => self.output_small,
            RxError::Metadata => self.metadata,
            RxError::Ignored => self.ignored,
            RxError::Unsupported => self.unsupported,
        }
    }
}

impl PrefixCounters {
    const fn new() -> Self {
        Self {
            invalid: AtomicU32::new(0),
            chain: AtomicU32::new(0),
            bounds: AtomicU32::new(0),
            rx_failure: AtomicU32::new(0),
            mic_failure: AtomicU32::new(0),
            quarantined: AtomicU32::new(0),
            output_small: AtomicU32::new(0),
            metadata: AtomicU32::new(0),
            ignored: AtomicU32::new(0),
            unsupported: AtomicU32::new(0),
            strict_rxend_refusable: AtomicU32::new(0),
            strict_dump_refusable: AtomicU32::new(0),
        }
    }

    fn report(&self) -> WifiRxPrefixReport {
        let load = |counter: &AtomicU32| counter.load(Ordering::Relaxed);
        WifiRxPrefixReport {
            invalid: load(&self.invalid),
            chain: load(&self.chain),
            bounds: load(&self.bounds),
            rx_failure: load(&self.rx_failure),
            mic_failure: load(&self.mic_failure),
            quarantined: load(&self.quarantined),
            output_small: load(&self.output_small),
            metadata: load(&self.metadata),
            ignored: load(&self.ignored),
            unsupported: load(&self.unsupported),
            strict_rxend_refusable: load(&self.strict_rxend_refusable),
            strict_dump_refusable: load(&self.strict_dump_refusable),
        }
    }

    const fn refused(&self, reason: RxError) -> &AtomicU32 {
        match reason {
            RxError::Invalid => &self.invalid,
            RxError::Chain => &self.chain,
            RxError::Bounds => &self.bounds,
            RxError::RxFailure => &self.rx_failure,
            RxError::MicFailure => &self.mic_failure,
            RxError::Quarantined => &self.quarantined,
            RxError::OutputSmall => &self.output_small,
            RxError::Metadata => &self.metadata,
            RxError::Ignored => &self.ignored,
            RxError::Unsupported => &self.unsupported,
        }
    }
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
    /// Units the publisher received.
    pub units: u32,
    /// Bytes of those units.
    pub bytes: u32,
    /// Chained units dropped.
    pub chained_drops: u32,
    /// Chains longer than one descriptor's length field, dropped.
    pub oversized_drops: u32,
    /// Empty units dropped.
    pub empty_drops: u32,
    /// Units longer than a staging lease, dropped.
    pub too_long_drops: u32,
    /// Bulk units dropped for want of an upper credit.
    pub overload_drops: u32,
    /// Units left at the ring head for want of a credit.
    pub admission_blocks: u32,
    /// Service passes which ended blocked on staging capacity.
    pub stage_capacity_blocks: u32,
    /// RX service transactions run.
    pub service_passes: u32,
    /// RX service transactions which failed.
    pub service_errors: u32,
    /// Continuations taken by an immediate self-wake.
    pub immediate_continuations: u32,
    /// Continuations taken after a timer wait.
    pub timed_continuations: u32,
    /// The most reload-doorbell observations one append made (a maximum, not a count).
    pub max_reload_attempts: u32,
    /// The application's RX-control prefix decode.
    pub prefix: WifiRxPrefixReport,
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
            units: AtomicU32::new(0),
            bytes: AtomicU32::new(0),
            chained_drops: AtomicU32::new(0),
            oversized_drops: AtomicU32::new(0),
            empty_drops: AtomicU32::new(0),
            too_long_drops: AtomicU32::new(0),
            overload_drops: AtomicU32::new(0),
            admission_blocks: AtomicU32::new(0),
            stage_capacity_blocks: AtomicU32::new(0),
            service_passes: AtomicU32::new(0),
            service_errors: AtomicU32::new(0),
            immediate_continuations: AtomicU32::new(0),
            timed_continuations: AtomicU32::new(0),
            max_reload_attempts: AtomicU32::new(0),
            prefix: PrefixCounters::new(),
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
            units: load(&self.units),
            bytes: load(&self.bytes),
            chained_drops: load(&self.chained_drops),
            oversized_drops: load(&self.oversized_drops),
            empty_drops: load(&self.empty_drops),
            too_long_drops: load(&self.too_long_drops),
            overload_drops: load(&self.overload_drops),
            admission_blocks: load(&self.admission_blocks),
            stage_capacity_blocks: load(&self.stage_capacity_blocks),
            service_passes: load(&self.service_passes),
            service_errors: load(&self.service_errors),
            immediate_continuations: load(&self.immediate_continuations),
            timed_continuations: load(&self.timed_continuations),
            max_reload_attempts: load(&self.max_reload_attempts),
            prefix: self.prefix.report(),
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

    pub(crate) fn received(&self, bytes: usize) {
        bump(&self.units);
        self.bytes
            .fetch_add(u32::try_from(bytes).unwrap_or(u32::MAX), Ordering::Relaxed);
    }

    pub(crate) fn discarded(&self, discard: Discard) {
        bump(match discard {
            Discard::Chained => &self.chained_drops,
            Discard::Oversized => &self.oversized_drops,
            Discard::Empty => &self.empty_drops,
            Discard::TooLong => &self.too_long_drops,
            Discard::OverloadBulk => &self.overload_drops,
        });
    }

    pub(crate) fn admission_blocked(&self) {
        bump(&self.admission_blocks);
    }

    pub(crate) fn stage_capacity_block(&self) {
        bump(&self.stage_capacity_blocks);
    }

    pub(crate) fn service_pass(&self) {
        bump(&self.service_passes);
    }

    pub(crate) fn service_error(&self) {
        bump(&self.service_errors);
    }

    pub(crate) fn immediate_continuation(&self) {
        bump(&self.immediate_continuations);
    }

    pub(crate) fn timed_continuation(&self) {
        bump(&self.timed_continuations);
    }

    pub(crate) fn prefix_refused(&self, reason: RxError) {
        bump(self.prefix.refused(reason));
    }

    pub(crate) fn strict_refusable(&self, rxend: bool, dump: bool) {
        if rxend {
            bump(&self.prefix.strict_rxend_refusable);
        }
        if dump {
            bump(&self.prefix.strict_dump_refusable);
        }
    }

    pub(crate) fn reload_attempts(&self, attempts: u32) {
        self.max_reload_attempts
            .fetch_max(attempts, Ordering::Relaxed);
    }
}
