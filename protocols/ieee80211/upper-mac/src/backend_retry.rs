//! Retry state behind the statically selected backend profile.
use crate::{
    AckFailureAccounting, MpduRetryState, RetryDecision, RetryLimits, RetryOutcome,
    retry::RetryStateError,
};
use oer_ieee80211_port::backend::*;
use oer_ieee80211_port::{TxStatus, VifRole};
use oer_ieee80211_wire::phy::{PhyRate, PpduBandwidth};

/// Retained MPDU retry state, parameterized by a static profile and backend values.
pub struct BackendMpduRetryState<X: BackendTxProfile, R: BackendTxRate, Q: BackendTxQueue> {
    role: VifRole,
    queue: Q,
    initial_rate: R,
    rate_policy: OrdinaryRetryRatePolicy,
    retry: MpduRetryState,
    profile: core::marker::PhantomData<X>,
}

impl<X: BackendTxProfile, R: BackendTxRate, Q: BackendTxQueue> BackendMpduRetryState<X, R, Q> {
    /// Start a retained MPDU using the profile's normal retry ladder.
    pub const fn new(
        role: VifRole,
        queue: Q,
        initial_rate: R,
        mpdu_retry_limit: u8,
        frame_class: FrameClass,
    ) -> Result<Self, OrdinaryRetryError<R>> {
        let retry = match MpduRetryState::new(
            RetryLimits {
                short: X::SHORT_RETRY_LIMIT,
                long: X::LONG_RETRY_LIMIT,
                ack_failure: if X::ACK_FAILURE_SHORT {
                    AckFailureAccounting::Short
                } else {
                    AckFailureAccounting::ByFrameClass
                },
            },
            mpdu_retry_limit,
            frame_class,
        ) {
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
            profile: core::marker::PhantomData,
        })
    }

    /// Construct an ordinary retry owner with an explicit rate policy.
    /// A management plan keeps its initial rate and standard short budget.
    /// A vendor record's first rate must match the published initial rate,
    /// preventing a caller from changing PHY only after the first ACK timeout.
    pub fn new_with_rate_policy(
        role: VifRole,
        queue: Q,
        initial_rate: R,
        rate_policy: OrdinaryRetryRatePolicy,
        mpdu_retry_limit: u8,
        frame_class: FrameClass,
    ) -> Result<Self, OrdinaryRetryError<R>> {
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
                let scheduled = select_schedule_retry_rate::<X, R>(schedule, 0)?;
                if scheduled != initial_rate {
                    return Err(OrdinaryRetryError::ScheduleInitialRateMismatch {
                        initial: initial_rate,
                        scheduled,
                    });
                }
            }
            OrdinaryRetryRatePolicy::P2p(schedule) => {
                let scheduled = select_p2p_retry_rate::<X, R>(schedule, 0)?;
                if scheduled != initial_rate {
                    return Err(OrdinaryRetryError::P2pInitialRateMismatch {
                        initial: initial_rate,
                        scheduled,
                    });
                }
            }
            OrdinaryRetryRatePolicy::P2pHtSgiFallback(schedule) => {
                let scheduled = select_p2p_retry_rate::<X, R>(schedule, 1)?;
                let valid = matches!(
                    (initial_rate.phy_rate(), scheduled.phy_rate()),
                    (PhyRate::Ht(initial), PhyRate::Ht(fallback))
                        if initial.bandwidth() == PpduBandwidth::Mhz20
                            && initial.short_gi()
                            && fallback.bandwidth() == PpduBandwidth::Mhz20
                            && !fallback.short_gi()
                            && fallback.mcs() == initial.mcs()
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

    /// Publications, including the current attempt.
    pub const fn publications(&self) -> u8 {
        self.retry.attempts()
    }

    /// Failed MPDU, short and long attempts of this owner.
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
    pub fn current_rate(&self) -> Result<R, OrdinaryRetryError<R>> {
        self.rate_after_failed_attempts(self.retry.counters().ladder_step())
    }

    /// Inspect one retry-series rate without advancing ownership.
    pub fn rate_after_failed_attempts(
        &self,
        failed_attempts: u8,
    ) -> Result<R, OrdinaryRetryError<R>> {
        match self.rate_policy {
            OrdinaryRetryRatePolicy::Management => Ok(self.initial_rate),
            OrdinaryRetryRatePolicy::Normal => select_ordinary_retry_rate::<X, R>(
                self.initial_rate,
                RetryCounters {
                    mpdu: failed_attempts,
                    short: failed_attempts,
                    long: 0,
                },
            ),
            OrdinaryRetryRatePolicy::Schedule(schedule) => {
                select_schedule_retry_rate::<X, R>(schedule, failed_attempts)
            }
            OrdinaryRetryRatePolicy::P2p(schedule) => {
                select_p2p_retry_rate::<X, R>(schedule, failed_attempts)
            }
            OrdinaryRetryRatePolicy::P2pHtSgiFallback(schedule) => {
                if failed_attempts == 0 {
                    Ok(self.initial_rate)
                } else {
                    select_p2p_retry_rate::<X, R>(schedule, failed_attempts)
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
        policy: &mut impl BackendContention<Q>,
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
        policy: &mut impl BackendContention<Q>,
    ) -> OrdinaryCollisionDecision {
        match self.observe_completion(policy, TxStatus::Collision) {
            OrdinaryRetryDecision::Complete => OrdinaryCollisionDecision::Complete,
            OrdinaryRetryDecision::Retry { .. } => OrdinaryCollisionDecision::Retry,
        }
    }

    /// End ownership after a non-retryable executor or hardware error.
    pub fn abort(&self, policy: &mut impl BackendContention<Q>) {
        policy.reset_terminal_exchange(self.role, self.queue);
    }
}

impl<X: BackendTxProfile, R: BackendTxRate, Q: BackendTxQueue> BackendMpduRetry<R, Q>
    for BackendMpduRetryState<X, R, Q>
{
    fn new(
        role: VifRole,
        queue: Q,
        initial: R,
        policy: OrdinaryRetryRatePolicy,
        limit: u8,
        class: FrameClass,
    ) -> Result<Self, OrdinaryRetryError<R>> {
        Self::new_with_rate_policy(role, queue, initial, policy, limit, class)
    }
    fn publications(&self) -> u8 {
        self.publications()
    }
    fn counters(&self) -> RetryCounters {
        self.counters()
    }
    fn current_rate(&self) -> Result<R, OrdinaryRetryError<R>> {
        self.current_rate()
    }
    fn rate_after_failed_attempts(&self, failures: u8) -> Result<R, OrdinaryRetryError<R>> {
        self.rate_after_failed_attempts(failures)
    }
    fn observe_completion<C: BackendContention<Q>>(
        &mut self,
        policy: &mut C,
        status: TxStatus,
    ) -> OrdinaryRetryDecision {
        self.observe_completion(policy, status)
    }
    fn observe_collision<C: BackendContention<Q>>(
        &mut self,
        policy: &mut C,
    ) -> OrdinaryCollisionDecision {
        self.observe_collision(policy)
    }
    fn abort<C: BackendContention<Q>>(&self, policy: &mut C) {
        self.abort(policy)
    }
}
/// Apply the normal profile ladder, preserving HT/HE rates without a record.
pub fn select_ordinary_retry_rate<X: BackendTxProfile, R: BackendTxRate>(
    initial: R,
    counters: RetryCounters,
) -> Result<R, OrdinaryRetryError<R>> {
    let retry_index = counters.mpdu.max(counters.short);
    let portable = initial.phy_rate();
    match X::retry_rate(portable, retry_index).and_then(R::from_phy_rate) {
        Some(rate) => Ok(rate),
        None if matches!(portable, PhyRate::Ht(_) | PhyRate::He(_)) => Ok(initial),
        None => Err(OrdinaryRetryError::RetryRateUnavailable { retry_index }),
    }
}
/// Select an exact P2P retry record.
pub fn select_p2p_retry_rate<X: BackendTxProfile, R: BackendTxRate>(
    schedule: P2pRetryRateSchedule,
    retry_index: u8,
) -> Result<R, OrdinaryRetryError<R>> {
    select_schedule_retry_rate::<X, R>(schedule.schedule(), retry_index)
}
/// Select an exact admitted record without a chip dependency.
pub fn select_schedule_retry_rate<X: BackendTxProfile, R: BackendTxRate>(
    schedule: RateScheduleRef,
    retry_index: u8,
) -> Result<R, OrdinaryRetryError<R>> {
    X::schedule_retry_rate(schedule, retry_index)
        .and_then(R::from_phy_rate)
        .ok_or(OrdinaryRetryError::RetryRateUnavailable { retry_index })
}
