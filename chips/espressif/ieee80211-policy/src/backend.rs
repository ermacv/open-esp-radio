//! Static Espressif profile for hardware backends.
use oer_ieee80211_port::backend::*;
use oer_ieee80211_port::backend_protection::*;
use oer_ieee80211_upper_mac::protection::{ProtectEveryHeTxop, ProtectionPolicy};
use oer_ieee80211_wire::{
    phy::{HeGiLtf, LegacyRate, PhyRate, PpduBandwidth},
    qos::WmmAccessCategory,
};

/// The recovered Espressif rate, retry and protection profile; it stores no state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspressifBackendTxProfile;
impl BackendTxProfile for EspressifBackendTxProfile {
    type MpduRetry<R: BackendTxRate, Q: BackendTxQueue> =
        oer_ieee80211_upper_mac::backend_retry::BackendMpduRetryState<Self, R, Q>;
    const ACK_FAILURE_SHORT: bool = true;
    const SHORT_RETRY_LIMIT: u8 = crate::lmac::SHORT_RETRY_LIMIT;
    const LONG_RETRY_LIMIT: u8 = crate::lmac::LONG_RETRY_LIMIT;
    const RTS_THRESHOLD_BYTES: u16 = crate::lmac::RTS_THRESHOLD_BYTES;
    const DEFAULT_CONTENTION: [DefaultContention; 4] = [
        crate::lmac::default_contention(WmmAccessCategory::BestEffort),
        crate::lmac::default_contention(WmmAccessCategory::Background),
        crate::lmac::default_contention(WmmAccessCategory::Video),
        crate::lmac::default_contention(WmmAccessCategory::Voice),
    ];
    fn rate_schedule(kind: RateScheduleKind, index: u8) -> Option<RateScheduleRef> {
        crate::rate_schedule::rate_schedule(kind, index)
    }
    fn schedule_state(schedule: RateScheduleRef) -> RateScheduleRecordState {
        crate::rate_schedule::schedule_state(schedule)
    }
    fn schedule_publication_limit(schedule: RateScheduleRef) -> u8 {
        crate::rate_schedule::schedule_publication_limit(schedule)
    }
    fn schedule_rate_after_failures(schedule: RateScheduleRef, failures: u8) -> Option<u8> {
        crate::rate_schedule::schedule_rate_after_failures(schedule, failures)
    }
    fn schedule_retry_rate(schedule: RateScheduleRef, failures: u8) -> Option<PhyRate> {
        crate::rate_code::phy_rate(
            RateScheduleKind::Dot11N,
            crate::rate_schedule::schedule_rate_after_failures(schedule, failures)?,
            PpduBandwidth::Mhz20,
            HeGiLtf::Ltf2xGi800Ns,
        )
    }
    fn retry_rate(initial: PhyRate, failures: u8) -> Option<PhyRate> {
        crate::retry_ladder::ordinary_retry_rate(initial, failures)
    }
    fn legacy_retry_publication_limit(initial: LegacyRate) -> Option<u8> {
        crate::retry_ladder::legacy_retry_publication_limit(initial)
    }
    fn select_protection(
        bss: BssProtection,
        threshold: Option<RtsLengthThreshold>,
        ppdu: ProtectedPpdu,
    ) -> TxProtectionDecision {
        policy(bss, threshold).select(ppdu, &crate::he_txop::EspressifHeTxopRtsBudget)
    }
    fn select_legacy_protection(
        bss: BssProtection,
        threshold: Option<RtsLengthThreshold>,
        rate: LegacyRate,
        receiver: TxReceiver,
        psdu: TxPsdu,
    ) -> TxProtectionDecision {
        policy(bss, threshold).select(
            ProtectedPpdu {
                rate: PhyRate::Legacy(rate),
                receiver,
                psdu,
            },
            &ProtectEveryHeTxop,
        )
    }
    fn control_rate(
        bss: BssProtection,
        threshold: Option<RtsLengthThreshold>,
        rate: PhyRate,
    ) -> LegacyRate {
        policy(bss, threshold).control_rate(rate)
    }
    fn maximum_unprotected_apep_bytes(
        threshold: HeTxopDurationRtsThreshold,
        padding: HePacketPadding,
        rate: oer_ieee80211_wire::phy::HeRate,
    ) -> u16 {
        crate::he_txop::maximum_unprotected_apep_bytes(threshold, padding, rate)
    }
}
fn policy(bss: BssProtection, threshold: Option<RtsLengthThreshold>) -> ProtectionPolicy {
    let mut policy = ProtectionPolicy::new(threshold);
    policy.install_bss(bss);
    policy
}

/// Espressif limits over the core's bounded aggregate retry owner.
#[repr(transparent)]
pub struct EspressifAmpduRetryState<const CAPACITY: usize>(
    oer_ieee80211_upper_mac::backend_aggregate::BoundedAmpduRetryState<CAPACITY>,
);
impl<const CAPACITY: usize> EspressifAmpduRetryState<CAPACITY> {
    /// Apply hardware facts after the queue has been detached.
    pub fn observe(
        &mut self,
        completion: impl Into<oer_ieee80211_port::backend_aggregate::BackendAmpduCompletion>,
        subframes: u8,
        now: oer_time::Instant,
        operational: bool,
        admitted: impl Fn(u8) -> Option<oer_time::Instant>,
    ) -> Result<
        oer_ieee80211_port::backend_aggregate::AmpduRetryDecision,
        oer_ieee80211_port::backend_aggregate::AmpduRetryError,
    > {
        self.0
            .observe(completion, subframes, now, operational, admitted)
    }

    /// Start a bounded aggregate owner with the Espressif retry limits.
    pub fn new(
        first: oer_ieee80211_wire::sequence::SequenceNumber,
        subframes: u8,
        policy: oer_ieee80211_port::backend_aggregate::AmpduRetryPolicy,
    ) -> Result<Self, oer_ieee80211_port::backend_aggregate::AmpduRetryError> {
        oer_ieee80211_upper_mac::backend_aggregate::BoundedAmpduRetryState::new(
            first,
            subframes,
            crate::lmac::ampdu_retry_policy(policy.lifetime, policy.retain_single_mpdu),
        )
        .map(Self)
    }
}
impl<const C: usize> core::ops::Deref for EspressifAmpduRetryState<C> {
    type Target = oer_ieee80211_upper_mac::backend_aggregate::BoundedAmpduRetryState<C>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<const C: usize> core::ops::DerefMut for EspressifAmpduRetryState<C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl<const C: usize> oer_ieee80211_port::backend_aggregate::BackendAmpduRetry
    for EspressifAmpduRetryState<C>
{
    fn current_first_sequence(&self) -> oer_ieee80211_wire::sequence::SequenceNumber {
        self.0.current_first_sequence()
    }
    fn observe(
        &mut self,
        completion: oer_ieee80211_port::backend_aggregate::BackendAmpduCompletion,
        subframes: u8,
        now: oer_time::Instant,
        operational: bool,
        admitted: impl Fn(u8) -> Option<oer_time::Instant>,
    ) -> Result<
        oer_ieee80211_port::backend_aggregate::AmpduRetryDecision,
        oer_ieee80211_port::backend_aggregate::AmpduRetryError,
    > {
        self.0
            .observe(completion, subframes, now, operational, admitted)
    }
}
