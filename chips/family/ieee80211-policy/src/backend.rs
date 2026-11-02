//! Espressif limits for the portable aggregate retry owner.

/// Espressif limits over the core's bounded aggregate retry owner.
#[repr(transparent)]
pub struct EspressifAmpduRetryState<const CAPACITY: usize>(
    oer_ieee80211_core_upper_mac::backend_aggregate::BoundedAmpduRetryState<CAPACITY>,
);
impl<const CAPACITY: usize> EspressifAmpduRetryState<CAPACITY> {
    /// Apply hardware facts after the queue has been detached.
    pub fn observe(
        &mut self,
        completion: impl Into<oer_ieee80211_contracts_port::backend_aggregate::BackendAmpduCompletion>,
        subframes: u8,
        now: oer_time::Instant,
        operational: bool,
        admitted: impl Fn(u8) -> Option<oer_time::Instant>,
    ) -> Result<
        oer_ieee80211_contracts_port::backend_aggregate::AmpduRetryDecision,
        oer_ieee80211_contracts_port::backend_aggregate::AmpduRetryError,
    > {
        self.0
            .observe(completion, subframes, now, operational, admitted)
    }

    /// Start a bounded aggregate owner with the Espressif retry limits.
    ///
    /// `lifetime` runs from an MSDU's admission; after it the MPDU is aged
    /// and discarded instead of retried. `retain_single_mpdu` keeps one
    /// missing MPDU in the aggregate: the recovered HE path requires it,
    /// because converting a one-member HE A-MPDU to the ordinary queue first
    /// needs the distinct `ppHEAMPDU2Normal` metadata transition, while the
    /// HT path sends one remaining MPDU through its ordinary retry owner.
    pub fn new(
        first: oer_ieee80211_wire::sequence::SequenceNumber,
        subframes: u8,
        lifetime: oer_time::Duration,
        retain_single_mpdu: bool,
    ) -> Result<Self, oer_ieee80211_contracts_port::backend_aggregate::AmpduRetryError> {
        oer_ieee80211_core_upper_mac::backend_aggregate::BoundedAmpduRetryState::new(
            first,
            subframes,
            crate::lmac::ampdu_retry_policy(lifetime, retain_single_mpdu),
        )
        .map(Self)
    }
}
impl<const C: usize> core::ops::Deref for EspressifAmpduRetryState<C> {
    type Target = oer_ieee80211_core_upper_mac::backend_aggregate::BoundedAmpduRetryState<C>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<const C: usize> core::ops::DerefMut for EspressifAmpduRetryState<C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl<const C: usize> oer_ieee80211_contracts_port::backend_aggregate::BackendAmpduRetry
    for EspressifAmpduRetryState<C>
{
    fn current_first_sequence(&self) -> oer_ieee80211_wire::sequence::SequenceNumber {
        self.0.current_first_sequence()
    }
    fn observe(
        &mut self,
        completion: oer_ieee80211_contracts_port::backend_aggregate::BackendAmpduCompletion,
        subframes: u8,
        now: oer_time::Instant,
        operational: bool,
        admitted: impl Fn(u8) -> Option<oer_time::Instant>,
    ) -> Result<
        oer_ieee80211_contracts_port::backend_aggregate::AmpduRetryDecision,
        oer_ieee80211_contracts_port::backend_aggregate::AmpduRetryError,
    > {
        self.0
            .observe(completion, subframes, now, operational, admitted)
    }
}
