//! Connected-STA ordinary outcomes, sampled only at terminal ownership edges.
//! Retry causes count re-publications, not the final failure or hardware retries.

use core::sync::atomic::{AtomicU32, Ordering};
use oer_esp32s31_ieee80211_mac::tx::TxCompletionDisposition;
use oer_esp32s31_ieee80211_runtime::diagnostics::aggregate_tx::OrdinaryTxOutcome;

pub(super) struct Counters {
    reported: AtomicU32,
    missing: AtomicU32,
    success: AtomicU32,
    cts_recovered: AtomicU32,
    cts_retries: AtomicU32,
    ack_retries: AtomicU32,
    collisions: AtomicU32,
    terminal_cts: AtomicU32,
    hardware_timeout: AtomicU32,
}
impl Counters {
    pub(super) const fn new() -> Self {
        Self {
            reported: AtomicU32::new(0),
            missing: AtomicU32::new(0),
            success: AtomicU32::new(0),
            cts_recovered: AtomicU32::new(0),
            cts_retries: AtomicU32::new(0),
            ack_retries: AtomicU32::new(0),
            collisions: AtomicU32::new(0),
            terminal_cts: AtomicU32::new(0),
            hardware_timeout: AtomicU32::new(0),
        }
    }
    pub(super) fn record(&self, outcome: Option<OrdinaryTxOutcome>) {
        let Some(outcome) = outcome else {
            self.missing.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let report = outcome.report();
        self.reported.fetch_add(1, Ordering::Relaxed);
        let success = matches!(outcome, OrdinaryTxOutcome::Success(_));
        self.success
            .fetch_add(u32::from(success), Ordering::Relaxed);
        self.cts_recovered.fetch_add(
            u32::from(success && report.retries.cts_timeouts != 0),
            Ordering::Relaxed,
        );
        self.cts_retries
            .fetch_add(u32::from(report.retries.cts_timeouts), Ordering::Relaxed);
        self.ack_retries
            .fetch_add(u32::from(report.retries.ack_timeouts), Ordering::Relaxed);
        self.collisions
            .fetch_add(u32::from(report.retries.collisions), Ordering::Relaxed);
        self.terminal_cts.fetch_add(
            u32::from(
                report
                    .completion
                    .is_some_and(|c| c.disposition() == TxCompletionDisposition::CtsTimeout),
            ),
            Ordering::Relaxed,
        );
        self.hardware_timeout.fetch_add(
            u32::from(matches!(outcome, OrdinaryTxOutcome::HardwareTimeout(_))),
            Ordering::Relaxed,
        );
    }
    pub(super) fn snapshot(&self) -> StationOrdinarySnapshot {
        StationOrdinarySnapshot {
            reported: self.reported.load(Ordering::Relaxed),
            missing: self.missing.load(Ordering::Relaxed),
            success: self.success.load(Ordering::Relaxed),
            cts_recovered: self.cts_recovered.load(Ordering::Relaxed),
            cts_retries: self.cts_retries.load(Ordering::Relaxed),
            ack_retries: self.ack_retries.load(Ordering::Relaxed),
            collisions: self.collisions.load(Ordering::Relaxed),
            terminal_cts: self.terminal_cts.load(Ordering::Relaxed),
            hardware_timeout: self.hardware_timeout.load(Ordering::Relaxed),
        }
    }
}

/// Diagnostic terminal counters for connected STA network ordinary MPDUs and
/// aggregate fallback. Excludes AP, management and hardware-generated responses.
/// Counters wrap at u32; deltas are valid within one wrap. CTS recovery means a
/// successful terminal result after a decoded CTS-timeout re-publication; it
/// does not by itself prove an on-air exchange or any Duration/NAV value.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StationOrdinarySnapshot {
    pub reported: u32,
    pub missing: u32,
    pub success: u32,
    pub cts_recovered: u32,
    pub cts_retries: u32,
    pub ack_retries: u32,
    pub collisions: u32,
    pub terminal_cts: u32,
    pub hardware_timeout: u32,
}
impl StationOrdinarySnapshot {
    pub(super) fn delta_since(self, earlier: Self) -> Self {
        Self {
            reported: self.reported.wrapping_sub(earlier.reported),
            missing: self.missing.wrapping_sub(earlier.missing),
            success: self.success.wrapping_sub(earlier.success),
            cts_recovered: self.cts_recovered.wrapping_sub(earlier.cts_recovered),
            cts_retries: self.cts_retries.wrapping_sub(earlier.cts_retries),
            ack_retries: self.ack_retries.wrapping_sub(earlier.ack_retries),
            collisions: self.collisions.wrapping_sub(earlier.collisions),
            terminal_cts: self.terminal_cts.wrapping_sub(earlier.terminal_cts),
            hardware_timeout: self.hardware_timeout.wrapping_sub(earlier.hardware_timeout),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_esp32s31_ieee80211_mac::tx::{
        LegacyRate, TxPhyRate,
        protection::{TxProtection, TxProtectionDecision, TxProtectionReasons},
    };
    use oer_esp32s31_ieee80211_runtime::diagnostics::aggregate_tx::{
        OrdinaryTxReport, OrdinaryTxRetryReport,
    };
    use oer_ieee80211_softmac::{MacTxResult, MacTxStatus};

    #[test]
    fn cts_recovery_is_distinct_from_ack_retry_timeout_and_missing_report() {
        let counters = Counters::new();
        let mut report = OrdinaryTxReport {
            status: MacTxStatus {
                result: MacTxResult::Transmitted,
                attempts: 2,
                final_rate: TxPhyRate::Legacy(LegacyRate::Ofdm24M),
                acknowledged: Some(true),
                ack_snr_db: None,
                airtime_micros: None,
            },
            completion: None,
            retries: OrdinaryTxRetryReport {
                cts_timeouts: 1,
                ack_timeouts: 0,
                collisions: 0,
            },
            // A CTS timeout exists only behind an RTS.
            protection: TxProtectionDecision {
                protection: TxProtection::RtsCts {
                    rate: LegacyRate::Ofdm24M,
                },
                reasons: TxProtectionReasons::LENGTH,
            },
        };
        counters.record(Some(OrdinaryTxOutcome::Success(report)));
        let earlier = counters.snapshot();
        assert_eq!(earlier.cts_recovered, 1);
        assert_eq!(earlier.cts_retries, 1);
        report.retries = OrdinaryTxRetryReport {
            cts_timeouts: 0,
            ack_timeouts: 2,
            collisions: 1,
        };
        counters.record(Some(OrdinaryTxOutcome::Success(report)));
        report.status.result = MacTxResult::HardwareTimeout;
        report.status.acknowledged = Some(false);
        report.retries = OrdinaryTxRetryReport {
            cts_timeouts: 3,
            ack_timeouts: 0,
            collisions: 0,
        };
        counters.record(Some(OrdinaryTxOutcome::HardwareTimeout(report)));
        counters.record(None);
        let delta = counters.snapshot().delta_since(earlier);
        assert_eq!(delta.reported, 2);
        assert_eq!(delta.missing, 1);
        assert_eq!(delta.success, 1);
        assert_eq!(delta.cts_recovered, 0);
        assert_eq!(delta.cts_retries, 3);
        assert_eq!(delta.ack_retries, 2);
        assert_eq!(delta.collisions, 1);
        assert_eq!(delta.hardware_timeout, 1);
        assert_eq!(
            counters.snapshot().delta_since(counters.snapshot()),
            StationOrdinarySnapshot::default()
        );
    }

    #[test]
    fn terminal_cts_failure_is_not_a_republication_or_recovery() {
        use oer_esp32s31_ieee80211_mac::tx::{TxCompletion, TxCookie};

        let counters = Counters::new();
        // Cross a counter wrap while retaining the terminal classification.
        counters.reported.store(u32::MAX, Ordering::Relaxed);
        let earlier = counters.snapshot();
        let completion = TxCompletion::new_model(TxCookie(1), 2, 0);
        counters.record(Some(OrdinaryTxOutcome::HardwareFailure(OrdinaryTxReport {
            status: MacTxStatus {
                result: MacTxResult::HardwareFailure(completion.tx_status()),
                attempts: 1,
                final_rate: TxPhyRate::Legacy(LegacyRate::Ofdm24M),
                acknowledged: Some(false),
                ack_snr_db: None,
                airtime_micros: None,
            },
            completion: Some(completion),
            retries: OrdinaryTxRetryReport::default(),
            protection: TxProtectionDecision::UNPROTECTED,
        })));
        assert_eq!(
            counters.snapshot().delta_since(earlier),
            StationOrdinarySnapshot {
                reported: 1,
                terminal_cts: 1,
                ..StationOrdinarySnapshot::default()
            }
        );
    }
}
