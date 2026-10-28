//! Espressif limits for the portable aggregate retry owner.

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
