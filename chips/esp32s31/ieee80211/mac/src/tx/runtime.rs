//! Executor-neutral transmit policy and bounded retry state.
//!
//! This module owns the finite policy between one detached hardware
//! completion and the next publication decision. It deliberately does not
//! wait for interrupts, access MMIO, mutate DMA storage or produce entropy;
//! those remain separate hardware/executor boundaries.

use oer_ieee80211_port::{TxStatus, VifRole};
use oer_ieee80211_wire::extensions::wmm::WmmParameterSet;
use oer_ieee80211_wire::qos::{
    WmmAccessCategory, WmmTrafficClass, WmmUserPriority, classify_ethernet_wmm,
};
use oer_ieee80211_wire::sequence::SequenceNumber;

use oer_espressif_ieee80211_policy::lmac;
use oer_ieee80211_upper_mac::{
    AmpduAttemptResult, AmpduRetryDecision as UpperAmpduRetryDecision,
    AmpduRetryError as UpperAmpduRetryError, AmpduRetryState as UpperAmpduRetryState,
    MpduRetryState, RetryDecision, RetryLimits, RetryOutcome, retry::RetryStateError,
};

use crate::{
    edca::{EdcaAccessPolicy, EdcaContentionParameters, EdcaParametersError, EdcaQueues},
    rate::schedule::{RateScheduleKind, RateScheduleRef, schedule_rate_after_failures},
    tx::ampdu::{HtAmpduTxCompletion, HtBlockAckObservation},
    tx::protection::{BssProtection, RtsLengthThreshold, WifiTxProtectionPolicy},
    tx::{HeEdcaTxopLimit, HtChannelWidth, HtPeerAmpduParameters, LegacyTxQueue, TxPhyRate},
};

const HARDWARE_BLOCK_ACK_WINDOW: usize = 32;

/// Result of applying the peer's negotiated ACM policy to one classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WmmAdmissionDisposition {
    /// The selected AC does not require an admission.
    AdmissionNotRequired,
    Downgraded {
        requested: WmmAccessCategory,
    },
    /// A non-QoS association owns only the legacy best-effort sequence space.
    NonQosBestEffort,
}

/// Complete station-side queue/TID policy selected for one network frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiTxTraffic {
    pub requested: WmmTrafficClass,
    pub user_priority: WmmUserPriority,
    pub access_category: WmmAccessCategory,
    pub admission: WmmAdmissionDisposition,
    txop_limit_units_32_us: u16,
}

impl WifiTxTraffic {
    pub const fn queue(self) -> LegacyTxQueue {
        LegacyTxQueue::from_access_category(self.access_category)
    }

    pub const fn tid(self) -> u8 {
        self.user_priority.value()
    }

    pub const fn txop_limit_units_32_us(self) -> u16 {
        self.txop_limit_units_32_us
    }

    /// Resolve the peer's negotiated HE duration budget and an optional
    /// integration ceiling without widening either value.
    pub const fn he_txop_limit(
        self,
        configured_ceiling: HeEdcaTxopLimit,
    ) -> Result<HeEdcaTxopLimit, WmmTxopUnsupported> {
        he_txop_limit(self.txop_limit_units_32_us, configured_ceiling)
    }

    /// HT aggregation has no reviewed negotiated-TXOP duration calculator.
    pub const fn require_ht_txop_support(self) -> Result<(), WmmTxopUnsupported> {
        require_ht_txop_support(self.txop_limit_units_32_us)
    }
}

/// The HE duration budget of an access category whose advertised TXOP
/// limit is `units_32_us`, under an optional integration ceiling, widening
/// neither.
pub const fn he_txop_limit(
    units_32_us: u16,
    configured_ceiling: HeEdcaTxopLimit,
) -> Result<HeEdcaTxopLimit, WmmTxopUnsupported> {
    let Some(negotiated) = HeEdcaTxopLimit::from_units_32_us(units_32_us) else {
        return Err(WmmTxopUnsupported::AdvertisedLimitTooWide { units_32_us });
    };
    if negotiated.is_default() {
        return Ok(configured_ceiling);
    }
    if configured_ceiling.is_default()
        || negotiated.units_32_us() <= configured_ceiling.units_32_us()
    {
        Ok(negotiated)
    } else {
        Ok(configured_ceiling)
    }
}

/// An HT aggregate keeps no TXOP limit: it is sent only in an access
/// category without one.
pub const fn require_ht_txop_support(units_32_us: u16) -> Result<(), WmmTxopUnsupported> {
    if units_32_us == 0 {
        Ok(())
    } else {
        Err(WmmTxopUnsupported::HtAggregateDurationBudget { units_32_us })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiTxTrafficError {
    /// Every possible AC at or below the requested priority requires an
    /// admission, while no TSPEC/ADDTS owner exists in this driver.
    AdmissionControlRequired { requested: WmmAccessCategory },
}

/// Unreviewed boundary which must not be inferred from WMM parsing alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WmmTxopUnsupported {
    AdvertisedLimitTooWide { units_32_us: u16 },
    HtAggregateDurationBudget { units_32_us: u16 },
    RtsCtsProtection,
    MultiPpduMediumOwnership,
}

/// Requests which would require an unproven hardware medium-ownership path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WmmHardwareMediumRequest {
    RtsCtsProtection,
    MultiPpduTxop,
}

/// Association-derived state used by all ordinary and aggregate TX paths.
///
/// The state is kept together so a caller cannot accidentally update a peer's
/// HT capability, BSS color and WMM contention policy through independent
/// ad-hoc fields. Entropy is supplied by the platform at the point where a
/// hardware queue is published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiTxRuntimePolicy {
    ht_ampdu: HtPeerAmpduParameters,
    he_bss_color: u8,
    /// Each interface's contention for the four queues: the station's,
    /// then the access point's.
    edca: [EdcaQueues; 2],
    protection: WifiTxProtectionPolicy,
}

impl WifiTxRuntimePolicy {
    /// Start with the same cold values as the complete vendor LMAC init.
    pub const fn vendor_defaults() -> Self {
        Self {
            ht_ampdu: HtPeerAmpduParameters::from_capability_byte(0),
            he_bss_color: 0,
            edca: [EdcaQueues::vendor_defaults(); 2],
            protection: WifiTxProtectionPolicy::new(Some(
                crate::tx::protection::RtsLengthThreshold::VENDOR_DEFAULT,
            )),
        }
    }

    const fn edca(&self, role: VifRole) -> &EdcaQueues {
        let [station, access_point] = &self.edca;
        match role {
            VifRole::Station => station,
            VifRole::AccessPoint => access_point,
        }
    }

    const fn edca_mut(&mut self, role: VifRole) -> &mut EdcaQueues {
        let [station, access_point] = &mut self.edca;
        match role {
            VifRole::Station => station,
            VifRole::AccessPoint => access_point,
        }
    }

    pub fn install_ht_ampdu(&mut self, parameters: HtPeerAmpduParameters) {
        self.ht_ampdu = parameters;
    }

    pub const fn ht_ampdu(&self) -> HtPeerAmpduParameters {
        self.ht_ampdu
    }

    /// Install the six-bit HE BSS color decoded from the peer's BSS Color IE.
    pub fn install_he_bss_color(&mut self, bss_color: u8) {
        self.he_bss_color =
            oer_ieee80211_wire::signal::BssColorCode::from_bytes([bss_color]).color();
    }

    pub const fn he_bss_color(&self) -> u8 {
        self.he_bss_color
    }

    /// Replace the BSS protection facts. Every later publication, including
    /// a retry of an active exchange, selects its protection from them.
    pub fn install_bss_protection(&mut self, bss: BssProtection) {
        self.protection.install_bss(bss);
    }

    /// Leave the BSS: only the local length threshold remains.
    pub fn clear_bss_protection(&mut self) {
        self.protection.clear_bss();
    }

    /// Replace the local dot11RTSThreshold; `None` disables the length rule.
    pub fn set_rts_length_threshold(&mut self, threshold: Option<RtsLengthThreshold>) {
        self.protection.set_rts_length_threshold(threshold);
    }

    pub const fn protection(&self) -> WifiTxProtectionPolicy {
        self.protection
    }

    /// Atomically validate and install all four WMM access categories of
    /// the interface of `role`; the other interface keeps its own.
    pub fn install_wmm(
        &mut self,
        role: VifRole,
        parameters: WmmParameterSet,
    ) -> Result<(), EdcaParametersError> {
        self.edca_mut(role).configure_from_wmm(parameters)
    }

    pub fn contention_parameters(
        &self,
        role: VifRole,
        queue: LegacyTxQueue,
    ) -> EdcaContentionParameters {
        self.edca(role).queue(queue).parameters()
    }

    pub const fn access_policy(&self, role: VifRole, queue: LegacyTxQueue) -> EdcaAccessPolicy {
        self.edca(role).access_policy(queue)
    }

    /// Classify and admit one network frame under the active peer WMM policy.
    ///
    /// With no QoS peer, all markings collapse to legacy BE. With QoS, an ACM
    /// bit is never treated as an admission: the request walks to a lower AC
    /// and rewrites its TID to that AC's canonical value. If even BK requires
    /// admission, the request fails closed.
    pub fn select_network_traffic(
        &self,
        role: VifRole,
        ethernet: &[u8],
        peer_qos: bool,
    ) -> Result<WifiTxTraffic, WifiTxTrafficError> {
        let requested = classify_ethernet_wmm(ethernet);
        if !peer_qos {
            let selected = self.edca(role).access_policy(LegacyTxQueue::BestEffort);
            return Ok(WifiTxTraffic {
                requested,
                user_priority: WmmUserPriority::UP0,
                access_category: WmmAccessCategory::BestEffort,
                admission: WmmAdmissionDisposition::NonQosBestEffort,
                txop_limit_units_32_us: selected.txop_limit_units_32_us(),
            });
        }

        let mut category = requested.access_category;
        loop {
            let selected = self
                .edca(role)
                .access_policy(LegacyTxQueue::from_access_category(category));
            if !selected.admission_control_mandatory() {
                let downgraded = category != requested.access_category;
                return Ok(WifiTxTraffic {
                    requested,
                    user_priority: if downgraded {
                        category.canonical_user_priority()
                    } else {
                        requested.user_priority
                    },
                    access_category: category,
                    admission: if downgraded {
                        WmmAdmissionDisposition::Downgraded {
                            requested: requested.access_category,
                        }
                    } else {
                        WmmAdmissionDisposition::AdmissionNotRequired
                    },
                    txop_limit_units_32_us: selected.txop_limit_units_32_us(),
                });
            }
            let Some(lower) = category.downgrade() else {
                return Err(WifiTxTrafficError::AdmissionControlRequired {
                    requested: requested.access_category,
                });
            };
            category = lower;
        }
    }

    /// Explicit frontier for hardware operations not established by the
    /// recovered ordinary/A-MPDU publication contract.
    pub const fn request_hardware_medium(
        &self,
        request: WmmHardwareMediumRequest,
    ) -> Result<(), WmmTxopUnsupported> {
        match request {
            WmmHardwareMediumRequest::RtsCtsProtection => Err(WmmTxopUnsupported::RtsCtsProtection),
            WmmHardwareMediumRequest::MultiPpduTxop => {
                Err(WmmTxopUnsupported::MultiPpduMediumOwnership)
            }
        }
    }

    #[cfg(any(test, feature = "test-support", oer_probe))]
    pub fn contention_exponent(&self, role: VifRole, queue: LegacyTxQueue) -> u8 {
        self.edca(role).queue(queue).current_exponent()
    }

    /// Select one hardware backoff slot from platform-provided entropy.
    pub fn select_backoff(&self, role: VifRole, queue: LegacyTxQueue, entropy: u32) -> u16 {
        self.edca(role).select_slot(queue, entropy)
    }

    pub fn record_retry_failure(&mut self, role: VifRole, queue: LegacyTxQueue) {
        self.edca_mut(role).record_retry_failure(queue);
    }

    pub fn record_success(&mut self, role: VifRole, queue: LegacyTxQueue) {
        self.edca_mut(role).record_success(queue);
    }

    pub fn reset_terminal_exchange(&mut self, role: VifRole, queue: LegacyTxQueue) {
        self.edca_mut(role).reset_terminal_exchange(queue);
    }
}

impl Default for WifiTxRuntimePolicy {
    fn default() -> Self {
        Self::vendor_defaults()
    }
}

/// Complete `lmacInit` defaults stored in `lmacConfMib[0x15]` and `[0x14]`:
/// the Espressif family's policy data of `oer_espressif_ieee80211_policy::lmac`.
pub const VENDOR_SHORT_RETRY_LIMIT: u8 = lmac::SHORT_RETRY_LIMIT;
pub const VENDOR_LONG_RETRY_LIMIT: u8 = lmac::LONG_RETRY_LIMIT;
pub const VENDOR_RTS_THRESHOLD_BYTES: u32 = lmac::RTS_THRESHOLD_BYTES as u32;

/// Invalid construction or a missing normal-schedule rate entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryRetryError {
    ZeroMpduRetryLimit,
    RetryRateUnavailable {
        retry_index: u8,
    },
    P2pInitialRateMismatch {
        initial: TxPhyRate,
        scheduled: TxPhyRate,
    },
    ScheduleInitialRateMismatch {
        initial: TxPhyRate,
        scheduled: TxPhyRate,
    },
    P2pHtSgiFallbackMismatch {
        initial: TxPhyRate,
        scheduled: TxPhyRate,
    },
}

/// One validated recovered standard-rate P2P retry record.
///
/// LR records are intentionally excluded: a retry policy cannot establish
/// the missing LR PLCP, receive-status and scoped PHY ownership contracts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct P2pRetryRateSchedule(RateScheduleRef);

impl P2pRetryRateSchedule {
    pub const fn new(schedule: RateScheduleRef) -> Option<Self> {
        if matches!(
            schedule.kind(),
            RateScheduleKind::P2pDot11G | RateScheduleKind::P2pDot11N
        ) {
            Some(Self(schedule))
        } else {
            None
        }
    }

    pub const fn schedule(self) -> RateScheduleRef {
        self.0
    }
}

/// Rate-ladder ownership for one retained ordinary MPDU.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OrdinaryRetryRatePolicy {
    /// Use the ordinary associated-station Dot11G/Dot11N ladder selected from
    /// the initial hardware rate.
    #[default]
    Normal,
    /// Keep the caller-derived management rate for every publication,
    /// within the standard short retry limit and the supplied budget.
    Management,
    /// Walk one explicitly selected rate-control record, such as the
    /// association's non-data schedule.
    Schedule(RateScheduleRef),
    /// Use one exact recovered P2P record for every retained publication.
    P2p(P2pRetryRateSchedule),
    /// Publish one explicitly selected HT20 SGI rate, then enter the exact
    /// same-MCS HT20 LGI P2P record after the first failed attempt.
    ///
    /// The recovered P2P arena contains an SGI record only for MCS7. The
    /// queue formatter nevertheless owns every HT20 SGI MCS0..7 code. This
    /// source-owned bridge does not invent a missing record: attempt zero is
    /// the caller's typed SGI rate, while every retry is selected from an
    /// existing LGI record at the original failure count. Construction below
    /// proves that the first scheduled retry is the same MCS at LGI.
    P2pHtSgiFallback(P2pRetryRateSchedule),
}

/// Driver-owned action after one ordinary MPDU attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryRetryDecision {
    Complete,
    /// Re-publish the same encoded MPDU. Only the ACK-timeout path sets the
    /// 802.11 Retry bit; CTS timeout and collision deliberately do not.
    Retry {
        set_retry_bit: bool,
    },
}

/// A collision never transmitted the MPDU and cannot request its Retry bit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryCollisionDecision {
    Complete,
    Retry,
}

pub use oer_ieee80211_upper_mac::{FrameClass, RetryCounters};

/// One bounded ordinary-MPDU retry transaction for the normal schedule path.
///
/// The caller retains the encoded MPDU and its DMA storage. The retry
/// counters, publication count and the retry decision are the portable
/// `oer_ieee80211_upper_mac::MpduRetryState` under the Espressif LMAC limits
/// (`oer_espressif_ieee80211_policy::lmac::RETRY_LIMITS`) or the management
/// plan's standard short budget; this adapter adds
/// the chip's rate ladders over [`TxPhyRate`] and applies the contention
/// changes to the queue's EDCA state.
pub struct OrdinaryMpduRetryState {
    role: VifRole,
    queue: LegacyTxQueue,
    initial_rate: TxPhyRate,
    rate_policy: OrdinaryRetryRatePolicy,
    retry: MpduRetryState,
}

impl OrdinaryMpduRetryState {
    pub const fn new(
        role: VifRole,
        queue: LegacyTxQueue,
        initial_rate: TxPhyRate,
        mpdu_retry_limit: u8,
        frame_class: FrameClass,
    ) -> Result<Self, OrdinaryRetryError> {
        let retry = match MpduRetryState::new(lmac::RETRY_LIMITS, mpdu_retry_limit, frame_class) {
            Ok(retry) => retry,
            Err(RetryStateError::ZeroMpduRetryLimit) => {
                return Err(OrdinaryRetryError::ZeroMpduRetryLimit);
            }
        };
        Ok(Self {
            role,
            queue,
            initial_rate,
            rate_policy: OrdinaryRetryRatePolicy::Normal,
            retry,
        })
    }

    /// Construct an ordinary retry owner with an explicit rate policy.
    /// A management plan keeps its initial rate and standard short budget.
    /// A vendor record's first rate must match the published initial rate,
    /// preventing a caller from changing PHY only after the first ACK timeout.
    pub fn new_with_rate_policy(
        role: VifRole,
        queue: LegacyTxQueue,
        initial_rate: TxPhyRate,
        rate_policy: OrdinaryRetryRatePolicy,
        mpdu_retry_limit: u8,
        frame_class: FrameClass,
    ) -> Result<Self, OrdinaryRetryError> {
        let mut state = Self::new(role, queue, initial_rate, mpdu_retry_limit, frame_class)?;
        match rate_policy {
            OrdinaryRetryRatePolicy::Normal => {}
            OrdinaryRetryRatePolicy::Management => {
                let limit = mpdu_retry_limit.min(RetryLimits::IEEE_DEFAULT.short);
                // Count every failed publication against one short budget,
                // including CTS failures and collisions. Broadcast's budget
                // is one; a mixture of failures cannot extend either plan.
                state.retry = MpduRetryState::new(
                    RetryLimits {
                        short: limit,
                        ..RetryLimits::IEEE_DEFAULT
                    },
                    limit,
                    FrameClass::Short,
                )
                .map_err(|_| OrdinaryRetryError::ZeroMpduRetryLimit)?;
            }
            OrdinaryRetryRatePolicy::Schedule(schedule) => {
                let scheduled = select_schedule_retry_rate(schedule, 0)?;
                if scheduled != initial_rate {
                    return Err(OrdinaryRetryError::ScheduleInitialRateMismatch {
                        initial: initial_rate,
                        scheduled,
                    });
                }
            }
            OrdinaryRetryRatePolicy::P2p(schedule) => {
                let scheduled = select_p2p_retry_rate(schedule, 0)?;
                if scheduled != initial_rate {
                    return Err(OrdinaryRetryError::P2pInitialRateMismatch {
                        initial: initial_rate,
                        scheduled,
                    });
                }
            }
            OrdinaryRetryRatePolicy::P2pHtSgiFallback(schedule) => {
                let scheduled = select_p2p_retry_rate(schedule, 1)?;
                let valid = matches!(
                    (initial_rate, scheduled),
                    (TxPhyRate::Ht(initial), TxPhyRate::Ht(fallback))
                        if initial.channel_width == HtChannelWidth::Mhz20
                            && initial.guard_interval
                                == crate::tx::HtGuardInterval::Short400Ns
                            && fallback.channel_width == HtChannelWidth::Mhz20
                            && fallback.guard_interval
                                == crate::tx::HtGuardInterval::Long800Ns
                            && fallback.mcs == initial.mcs
                );
                if !valid {
                    return Err(OrdinaryRetryError::P2pHtSgiFallbackMismatch {
                        initial: initial_rate,
                        scheduled,
                    });
                }
            }
        }
        state.rate_policy = rate_policy;
        Ok(state)
    }

    pub const fn publications(&self) -> u8 {
        self.retry.attempts()
    }

    pub const fn counters(&self) -> RetryCounters {
        self.retry.counters()
    }

    /// Select the rate for the current publication.
    ///
    /// Both normal and P2P records use the `rcGetRate` retry counters. The
    /// descriptor bypass and context-fixed-rate branches remain separate
    /// production modes and are not inferred here. Selection uses
    /// `max(desc[5], desc[6])`; a long collision changes only `desc[7]` and
    /// therefore retains its current rate.
    pub fn current_rate(&self) -> Result<TxPhyRate, OrdinaryRetryError> {
        self.rate_after_failed_attempts(self.retry.counters().ladder_step())
    }

    /// Inspect one retry-series rate without advancing ownership.
    pub fn rate_after_failed_attempts(
        &self,
        failed_attempts: u8,
    ) -> Result<TxPhyRate, OrdinaryRetryError> {
        match self.rate_policy {
            OrdinaryRetryRatePolicy::Management => Ok(self.initial_rate),
            OrdinaryRetryRatePolicy::Normal => select_ordinary_retry_rate(
                self.initial_rate,
                RetryCounters {
                    mpdu: failed_attempts,
                    short: failed_attempts,
                    long: 0,
                },
            ),
            OrdinaryRetryRatePolicy::Schedule(schedule) => {
                select_schedule_retry_rate(schedule, failed_attempts)
            }
            OrdinaryRetryRatePolicy::P2p(schedule) => {
                select_p2p_retry_rate(schedule, failed_attempts)
            }
            OrdinaryRetryRatePolicy::P2pHtSgiFallback(schedule) => {
                if failed_attempts == 0 {
                    Ok(self.initial_rate)
                } else {
                    select_p2p_retry_rate(schedule, failed_attempts)
                }
            }
        }
    }

    /// Apply one typed completion after the raw status/detail dispatcher.
    ///
    /// The portable retry state decides: only an ACK timeout sets the Retry
    /// bit, a CTS timeout advances the short counter, a collision the
    /// counter of the frame's class, and a terminal failure ends the
    /// exchange.
    pub fn observe_completion(
        &mut self,
        policy: &mut WifiTxRuntimePolicy,
        status: TxStatus,
    ) -> OrdinaryRetryDecision {
        // The frame's end resets the queue's contention window and a retry
        // doubles it.
        let decision = self.retry.observe(status);
        match decision {
            RetryDecision::Complete(RetryOutcome::Delivered) => {
                policy.record_success(self.role, self.queue);
            }
            RetryDecision::Complete(_) => policy.reset_terminal_exchange(self.role, self.queue),
            RetryDecision::Retry { .. } => policy.record_retry_failure(self.role, self.queue),
        }
        match decision {
            RetryDecision::Complete(_) => OrdinaryRetryDecision::Complete,
            RetryDecision::Retry { set_retry_bit } => {
                OrdinaryRetryDecision::Retry { set_retry_bit }
            }
        }
    }

    /// Apply one detached ordinary-queue collision.
    pub fn observe_collision(
        &mut self,
        policy: &mut WifiTxRuntimePolicy,
    ) -> OrdinaryCollisionDecision {
        match self.observe_completion(policy, TxStatus::Collision) {
            OrdinaryRetryDecision::Complete => OrdinaryCollisionDecision::Complete,
            OrdinaryRetryDecision::Retry { .. } => OrdinaryCollisionDecision::Retry,
        }
    }

    /// End ownership after a non-retryable executor or hardware error.
    pub fn abort(&self, policy: &mut WifiTxRuntimePolicy) {
        policy.reset_terminal_exchange(self.role, self.queue);
    }
}

/// Exact production rate-selection entry for the normal `rcGetRate` slice.
///
/// Keeping this as a named, non-inlined entry lets vendor comparison execute
/// the same compiled function used by [`OrdinaryMpduRetryState`] rather than
/// a shadow retry model.
#[inline(never)]
pub fn select_ordinary_retry_rate(
    initial_rate: TxPhyRate,
    counters: RetryCounters,
) -> Result<TxPhyRate, OrdinaryRetryError> {
    let retry_index = counters.mpdu.max(counters.short);
    match initial_rate {
        TxPhyRate::Legacy(rate) => rate
            .vendor_retry_rate(retry_index)
            .map(TxPhyRate::Legacy)
            .ok_or(OrdinaryRetryError::RetryRateUnavailable { retry_index }),
        TxPhyRate::Ht(rate) => Ok(rate
            .vendor_retry_rate(retry_index)
            .unwrap_or(TxPhyRate::Ht(rate))),
        // An HE single-MPDU walks its 802.11ax record like HT, and may leave
        // the HE domain; a rate without a record keeps its rate.
        TxPhyRate::He(rate) => Ok(rate
            .vendor_retry_rate(retry_index)
            .unwrap_or(TxPhyRate::He(rate))),
    }
}

/// Select one attempt from an exact standard P2P record.
///
/// The dedicated P2P arenas never enter the proprietary LR or HE rate-code
/// domains. HT values are decoded as 20-MHz one-stream rates; MCS32 therefore
/// cannot be introduced through this path.
#[inline(never)]
pub fn select_p2p_retry_rate(
    schedule: P2pRetryRateSchedule,
    retry_index: u8,
) -> Result<TxPhyRate, OrdinaryRetryError> {
    select_schedule_retry_rate(schedule.schedule(), retry_index)
}

/// Select one `rcGetRate` attempt of an exact recovered record. Rates decode
/// as 20-MHz one-stream values, as for the P2P records.
#[inline(never)]
pub fn select_schedule_retry_rate(
    schedule: RateScheduleRef,
    retry_index: u8,
) -> Result<TxPhyRate, OrdinaryRetryError> {
    let code = schedule_rate_after_failures(schedule, retry_index)
        .ok_or(OrdinaryRetryError::RetryRateUnavailable { retry_index })?;
    TxPhyRate::from_code(code, HtChannelWidth::Mhz20)
        .ok_or(OrdinaryRetryError::RetryRateUnavailable { retry_index })
}

/// Lifetime of an A-MPDU member MSDU, in microseconds: 1536 lifetime units
/// of 1024 us (`oer_espressif_ieee80211_policy::lmac`).
pub const VENDOR_AMPDU_MSDU_LIFETIME_MICROS: u32 = lmac::AMPDU_MSDU_LIFETIME_MICROS;

/// Policy for retained A-MPDU retries.
///
/// As the vendor's `ppResortTxAMPDU` does, a partial BlockAck keeps the
/// missing MPDUs in the aggregate without counting publications; only their
/// MSDU lifetime bounds the retries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduRetryPolicy {
    /// Time from an MSDU's admission after which its MPDU is aged and
    /// discarded instead of retried.
    pub lifetime: oer_time::Duration,
    /// Keep one missing MPDU in the aggregate owner.
    ///
    /// The recovered HE path requires this because converting a one-member
    /// HE A-MPDU to the ordinary queue first needs the distinct
    /// `ppHEAMPDU2Normal` metadata transition. The qualified HT path instead
    /// sends one remaining MPDU through its ordinary retry owner.
    pub retain_single_mpdu: bool,
}

/// Invalid construction or a disagreement with the pinned DMA owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRetryError {
    ZeroLifetime,
    /// The caller has no admission instant for this original MPDU.
    UnknownSubframe {
        index: u8,
    },
    EmptyAggregate,
    CapacityExceedsHardwareWindow {
        capacity: usize,
    },
    AggregateExceedsCapacity {
        subframes: u8,
        capacity: usize,
    },
    FrameCountChanged {
        expected: u8,
        observed: u8,
    },
}

/// Driver-owned action after one BlockAck completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRetryDecision {
    /// Retain and compact the selected MPDUs, then publish another A-MPDU.
    RetainAggregate { retry_mask: u32 },
    /// The protection exchange failed before the data PPDU: publish the same
    /// aggregate again. No MPDU was transmitted, so none gains the Retry bit.
    ///
    /// SOURCE: complete `libpp[lmac.o]::lmacProcessCtsTimeout` calls
    /// `libpp[lmac.o]::lmacProcessShortRetryFail(queue, 0, 1)`, which advances the short
    /// retry count and contention window, skips the Retry-bit leaf and
    /// re-enters `libpp[lmac.o]::lmacEndFrameExchangeSequence` with the unchanged frame.
    RepublishUnchanged { retry_mask: u32 },
    /// The protection exchange failed at every attempt: keep the aggregate
    /// and send a BlockAckReq starting at its head. The BlockAck it solicits
    /// drives the resort ([`AmpduRetryState::observe_block_ack_request`]).
    ///
    /// SOURCE: `libpp[lmac.o]::lmacEndRetryAMPDUFail` keeps an
    /// RTS-protected aggregate whose CTS never arrived and sends a
    /// BlockAckReq through `libpp[pp.o]::ppFillAMPDUBar`/`libpp[pp.o]::ppReSendBar` with the starting
    /// sequence of its head.
    RequestBlockAck {
        retry_mask: u32,
        starting_sequence: SequenceNumber,
    },
    /// End aggregate ownership and send the selected MPDUs individually,
    /// in order, each already carrying its Retry bit.
    ///
    /// SOURCE: `libpp[pp.o]::ppResortTxAMPDU` moves the remaining MPDUs
    /// to the head of the queue's ordinary list when
    /// `libpp[trc.o]::trc_isTxAmpduOperational` or `libpp[trc.o]::trc_tid_isTxAmpduOperational` is
    /// false after its resort, first converting a missing aggregate head
    /// through `libpp[pp_he.o]::ppHEAMPDU2Normal`. A single missing HT
    /// MPDU takes the same ordinary path.
    Unaggregate { retry_mask: u32 },
    /// Missing MPDUs aged under an operational Block Ack agreement: compact
    /// the aggregate to the selected MPDUs, discard those of `aged_mask` and
    /// send a BlockAckReq at `starting_sequence`, whose BlockAck drives the
    /// resort ([`AmpduRetryState::observe_block_ack_request`]). Every MPDU
    /// ages from its MSDU's admission, not from the aggregate's commit as in
    /// `libpp[lmac.o]::lmacMSDUAged` (`docs/decisions.md`, MSDU age).
    DiscardAged {
        retry_mask: u32,
        aged_mask: u32,
        starting_sequence: SequenceNumber,
    },
    /// End aggregate ownership. The selected MPDUs were not acknowledged
    /// and are discarded.
    Finish { retry_mask: u32 },
    /// End ownership through the vendor Trigger-based completion path.
    ///
    /// No ordinary BlockAck was received, so this must remain distinct from
    /// `Finish { retry_mask: 0 }` for statistics and rate-control purposes.
    FinishTriggerFlow,
}

impl AmpduRetryDecision {
    pub const fn retry_mask(self) -> u32 {
        match self {
            Self::RetainAggregate { retry_mask }
            | Self::RepublishUnchanged { retry_mask }
            | Self::RequestBlockAck { retry_mask, .. }
            | Self::DiscardAged { retry_mask, .. }
            | Self::Unaggregate { retry_mask }
            | Self::Finish { retry_mask } => retry_mask,
            Self::FinishTriggerFlow => 0,
        }
    }

    pub const fn missing(self) -> u8 {
        self.retry_mask().count_ones() as u8
    }
}

/// One bounded BlockAck/retry transaction.
///
/// The retry selection is the portable
/// `oer_ieee80211_upper_mac::AmpduRetryState` under the Espressif LMAC retry
/// limit and aging margin (`oer_espressif_ieee80211_policy::lmac`). This
/// adapter bounds the aggregate by the S31 hardware BlockAck window and the
/// DMA owner's `CAPACITY`, reads the chip completion and keeps the 32-bit
/// masks of the S31 descriptor chain.
///
/// SOURCE: complete `libpp[pp.o]::ppResortTxAMPDU` preserves Sequence
/// Control and compacts only the MPDUs absent from BlockAck. Complete
/// `libpp[lmac.o]::lmacRetryTxFrame` skips `libpp[trc.o]::rcGetRate` for the state
/// written by `libpp[lmac.o]::lmacProcessLongRetryFail`, so a retained aggregate keeps its
/// PHY rate.
pub struct AmpduRetryState<const CAPACITY: usize> {
    state: UpperAmpduRetryState,
}

impl<const CAPACITY: usize> AmpduRetryState<CAPACITY> {
    /// Start at the first Sequence Control value already consumed by the
    /// encoded aggregate. Each MPDU ages from its MSDU's admission on the
    /// monotonic clock, which the caller gives with every completion.
    pub fn new(
        first_sequence: SequenceNumber,
        subframes: u8,
        policy: AmpduRetryPolicy,
    ) -> Result<Self, AmpduRetryError> {
        if policy.lifetime.as_micros() == 0 {
            return Err(AmpduRetryError::ZeroLifetime);
        }
        if CAPACITY > HARDWARE_BLOCK_ACK_WINDOW {
            return Err(AmpduRetryError::CapacityExceedsHardwareWindow { capacity: CAPACITY });
        }
        if subframes == 0 {
            return Err(AmpduRetryError::EmptyAggregate);
        }
        if usize::from(subframes) > CAPACITY {
            return Err(AmpduRetryError::AggregateExceedsCapacity {
                subframes,
                capacity: CAPACITY,
            });
        }
        UpperAmpduRetryState::new(
            first_sequence,
            subframes,
            lmac::ampdu_retry_policy(policy.lifetime, policy.retain_single_mpdu),
        )
        .map(|state| Self { state })
        .map_err(Self::error)
    }

    /// Apply one completion after the hardware queue has been detached.
    ///
    /// A missing MPDU stays in the aggregate until it is aged at monotonic
    /// `now`, `admitted` giving its MSDU's admission; aged MPDUs are
    /// discarded, behind a BlockAckReq under the agreement. Once
    /// the TID's BlockAck agreement is no longer `block_ack_operational`,
    /// the live missing MPDUs leave the aggregate for individual retry.
    ///
    /// SOURCE: complete `libpp[lmac.o]::lmacProcessTxComplete` maps status
    /// five to `libpp[lmac.o]::lmacProcessAckTimeout`. Both its short- and long-frame
    /// leaves call `lmacProcessTBSuccess(queue, 0x7f)` instead of retrying
    /// when the queue is in Trigger flow and its applicable packet counts
    /// are zero ([`crate::tx::TxCompletion::completes_vendor_trigger_flow`]),
    /// which ends the exchange without an ordinary BlockAck. A CTS timeout is
    /// a failed protection exchange (`libpp[lmac.o]::lmacProcessCtsTimeout`). Without the
    /// hardware's BlockAck-received result every MPDU is retried; with it,
    /// the bitmap under [`HtAmpduTxCompletion::valid_block_ack`] resorts the
    /// aggregate.
    pub fn observe(
        &mut self,
        completion: HtAmpduTxCompletion,
        observed_subframes: u8,
        now: oer_time::Instant,
        block_ack_operational: bool,
        admitted: impl Fn(u8) -> Option<oer_time::Instant>,
    ) -> Result<AmpduRetryDecision, AmpduRetryError> {
        let result = if completion.tx.completes_vendor_trigger_flow() {
            AmpduAttemptResult::TriggerFlowEnd
        } else if completion.tx.tx_status() == TxStatus::CtsTimeout {
            AmpduAttemptResult::ProtectionFailure
        } else if !completion.block_ack_received {
            AmpduAttemptResult::NoResponse
        } else {
            AmpduAttemptResult::Answered(
                completion
                    .valid_block_ack()
                    .map(|block_ack| block_ack.report()),
            )
        };
        self.state
            .observe(
                result,
                observed_subframes,
                now,
                block_ack_operational,
                admitted,
            )
            .map(Self::decision)
            .map_err(Self::error)
    }

    /// Resort the kept aggregate by the answer to its BlockAckReq.
    ///
    /// `block_ack` is the BlockAck received for the request, or `None` when
    /// the request exhausted its retries unanswered: the resort then keeps
    /// every MPDU as missing, as the vendor resorts its unchanged queue
    /// record.
    pub fn observe_block_ack_request(
        &mut self,
        block_ack: Option<HtBlockAckObservation>,
        now: oer_time::Instant,
        block_ack_operational: bool,
        admitted: impl Fn(u8) -> Option<oer_time::Instant>,
    ) -> Result<AmpduRetryDecision, AmpduRetryError> {
        self.state
            .observe_block_ack_request(
                block_ack.map(|observation| observation.block_ack.report()),
                now,
                block_ack_operational,
                admitted,
            )
            .map(Self::decision)
            .map_err(Self::error)
    }

    const fn decision(decision: UpperAmpduRetryDecision) -> AmpduRetryDecision {
        // The S31 aggregate holds at most `HARDWARE_BLOCK_ACK_WINDOW`
        // subframes, so every mask fits the descriptor chain's 32 bits.
        match decision {
            UpperAmpduRetryDecision::RetainAggregate { retry_mask } => {
                AmpduRetryDecision::RetainAggregate {
                    retry_mask: retry_mask as u32,
                }
            }
            UpperAmpduRetryDecision::RepublishUnchanged { retry_mask } => {
                AmpduRetryDecision::RepublishUnchanged {
                    retry_mask: retry_mask as u32,
                }
            }
            UpperAmpduRetryDecision::RequestBlockAck {
                retry_mask,
                starting_sequence,
            } => AmpduRetryDecision::RequestBlockAck {
                retry_mask: retry_mask as u32,
                starting_sequence,
            },
            UpperAmpduRetryDecision::DiscardAged {
                retry_mask,
                aged_mask,
                starting_sequence,
            } => AmpduRetryDecision::DiscardAged {
                retry_mask: retry_mask as u32,
                aged_mask: aged_mask as u32,
                starting_sequence,
            },
            UpperAmpduRetryDecision::Unaggregate { retry_mask } => {
                AmpduRetryDecision::Unaggregate {
                    retry_mask: retry_mask as u32,
                }
            }
            UpperAmpduRetryDecision::Finish { retry_mask } => AmpduRetryDecision::Finish {
                retry_mask: retry_mask as u32,
            },
            UpperAmpduRetryDecision::FinishTriggerFlow => AmpduRetryDecision::FinishTriggerFlow,
        }
    }

    const fn error(error: UpperAmpduRetryError) -> AmpduRetryError {
        match error {
            UpperAmpduRetryError::UnknownSubframe { index } => {
                AmpduRetryError::UnknownSubframe { index }
            }
            UpperAmpduRetryError::ZeroLifetime => AmpduRetryError::ZeroLifetime,
            UpperAmpduRetryError::EmptyAggregate => AmpduRetryError::EmptyAggregate,
            UpperAmpduRetryError::TooManySubframes { subframes } => {
                AmpduRetryError::AggregateExceedsCapacity {
                    subframes,
                    capacity: CAPACITY,
                }
            }
            UpperAmpduRetryError::FrameCountChanged { expected, observed } => {
                AmpduRetryError::FrameCountChanged { expected, observed }
            }
        }
    }

    pub const fn current_subframes(&self) -> u8 {
        self.state.current_subframes()
    }

    /// Whether an MSDU admitted at `admitted` is aged at `now`: less than
    /// one lifetime unit remains.
    pub fn aged(&self, admitted: oer_time::Instant, now: oer_time::Instant) -> bool {
        self.state.aged(admitted, now)
    }

    /// Where the BlockAckReq of the last `RequestBlockAck` or
    /// `DiscardAged` decision starts.
    pub const fn block_ack_request_start(&self) -> SequenceNumber {
        self.state.block_ack_request_start()
    }

    /// Original aggregate positions discarded as aged.
    pub const fn discarded_original_indices(&self) -> u32 {
        self.state.discarded_original_indices() as u32
    }

    pub const fn current_first_sequence(&self) -> SequenceNumber {
        self.state.current_first_sequence()
    }

    /// Original aggregate positions (bit `i` is the `i`th MPDU of the first
    /// publication) absent from the last observed completion. Retries
    /// compact the descriptor chain; these positions do not move.
    pub const fn missing_original_indices(&self) -> u32 {
        self.state.missing_original_indices() as u32
    }

    pub const fn aggregate_attempts(&self) -> u8 {
        self.state.aggregate_attempts()
    }

    pub const fn acknowledged(&self) -> u8 {
        self.state.acknowledged()
    }

    pub const fn block_ack_mpdu_attempts(&self) -> u16 {
        self.state.block_ack_mpdu_attempts()
    }

    /// Protection exchanges (RTS/CTS) that failed before the data PPDU.
    pub const fn protection_failures(&self) -> u8 {
        self.state.protection_failures()
    }

    /// Number of terminal completions handled through `lmacProcessTBSuccess`
    /// semantics rather than an ordinary BlockAck.
    pub const fn trigger_flow_completions(&self) -> u8 {
        self.state.trigger_flow_completions()
    }
}

#[cfg(test)]
mod tests;
