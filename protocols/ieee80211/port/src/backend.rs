//! Static transmit profiles and the values supplied to hardware backends.
use crate::PhyRate;

/// Octets in a profile-owned opaque rate record.
pub const RATE_SCHEDULE_RECORD_SIZE: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
/// Format family of one profile-owned rate-schedule record.
pub enum RateScheduleKind {
    // Deliberately sparse internal tags keep LLVM from materializing a
    // nine-pointer jump table in internal SRAM.  These values never cross the
    // source-owned schedule identity.
    /// OFDM control exchange record.
    BarOfdm = 0x03,
    /// Basic OFDM record.
    BasicOfdm = 0x19,
    /// HE record.
    Dot11Ax = 0x2f,
    /// DSSS/CCK record.
    Dot11B = 0x44,
    /// Non-HT OFDM record.
    Dot11G = 0x62,
    /// HT record.
    Dot11N = 0x83,
    /// Espressif LR record.
    Lora = 0xa1,
    /// Dedicated P2P OFDM record.
    P2pDot11G = 0xc8,
    /// Dedicated P2P HT record.
    P2pDot11N = 0xf1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Immutable record identity supplied by a transmit profile; contains no retry state.
pub struct RateScheduleRef {
    kind: RateScheduleKind,
    record: &'static [u8; RATE_SCHEDULE_RECORD_SIZE],
}

impl RateScheduleRef {
    /// Bind one profile-owned immutable schedule record to its format family.
    pub const fn from_record(
        kind: RateScheduleKind,
        record: &'static [u8; RATE_SCHEDULE_RECORD_SIZE],
    ) -> Self {
        Self { kind, record }
    }
    /// The exact immutable record supplied by the profile.
    pub const fn record(self) -> &'static [u8; RATE_SCHEDULE_RECORD_SIZE] {
        self.record
    }
    /// The profile record format family.
    pub const fn kind(self) -> RateScheduleKind {
        self.kind
    }
    /// The record index carried in its source format.
    pub const fn index(self) -> u8 {
        let [_, _, _, _, _, _, _, _, _, _, index, _] = self.record;
        *index
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Decoded record values supplied by the profile.
pub struct RateScheduleRecordState {
    /// Initial rate code in the profile domain.
    pub rate: u8,
    /// Publication budget carried by the record.
    pub retry_limit: u8,
    /// Record index used by adaptive rate control.
    pub index: u8,
    /// Profile-specific adaptive rate-control selector.
    pub adaptive: u8,
}

/// Which retry counter a collision of the frame advances: IEEE 802.11
/// classifies a frame as long when it is longer than dot11RTSThreshold.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FrameClass {
    #[default]
    /// Frame within dot11RTSThreshold.
    Short,
    /// Frame exceeding dot11RTSThreshold.
    Long,
}

/// The counters of one frame's failed attempts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RetryCounters {
    /// Unacknowledged transmissions of the frame itself.
    pub mpdu: u8,
    /// Failures charged to the short retry counter.
    pub short: u8,
    /// Failures charged to the long retry counter.
    pub long: u8,
}

impl RetryCounters {
    /// Failures that walk the rate ladder: every ACK timeout and every
    /// failure the short counter records. A long-frame collision leaves the
    /// rate where it is.
    pub const fn ladder_step(self) -> u8 {
        if self.mpdu > self.short {
            self.mpdu
        } else {
            self.short
        }
    }
}

/// Invalid construction or a missing normal-schedule rate entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryRetryError<R> {
    /// No publication is allowed by the supplied budget.
    ZeroMpduRetryLimit,
    /// The profile cannot supply a rate at this retry step.
    RetryRateUnavailable {
        /// Failed-attempt count whose rate was unavailable.
        retry_index: u8,
    },
    /// The P2P record disagrees with the admitted initial rate.
    P2pInitialRateMismatch {
        /// The caller-admitted initial rate.
        initial: R,
        /// The rate selected by the supplied profile record.
        scheduled: R,
    },
    /// The explicit record disagrees with the admitted initial rate.
    ScheduleInitialRateMismatch {
        /// The caller-admitted initial rate.
        initial: R,
        /// The rate selected by the supplied profile record.
        scheduled: R,
    },
    /// The fallback record does not preserve the SGI request's MCS at LGI.
    P2pHtSgiFallbackMismatch {
        /// The caller-admitted initial rate.
        initial: R,
        /// The rate selected by the supplied profile record.
        scheduled: R,
    },
}

/// One validated recovered standard-rate P2P retry record.
///
/// LR records are intentionally excluded: a retry policy cannot establish
/// the missing LR PLCP, receive-status and scoped PHY ownership contracts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct P2pRetryRateSchedule(RateScheduleRef);

impl P2pRetryRateSchedule {
    /// Admit only a dedicated P2P OFDM or HT record.
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

    /// The exact admitted P2P record.
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
    /// End ownership of the retained MPDU.
    Complete,
    /// Re-publish the same encoded MPDU. Only the ACK-timeout path sets the
    /// 802.11 Retry bit; CTS timeout and collision deliberately do not.
    Retry {
        /// Set the on-air Retry bit for an unacknowledged MPDU.
        set_retry_bit: bool,
    },
}

/// A collision never transmitted the MPDU and cannot request its Retry bit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryCollisionDecision {
    /// End ownership of the retained MPDU.
    Complete,
    /// Republish the retained MPDU without setting its Retry bit.
    Retry,
}

/// A backend's lossless representation of an admitted portable PHY rate.
pub trait BackendTxRate: Copy + core::fmt::Debug + Eq {
    /// The portable rate represented by this hardware value.
    fn phy_rate(self) -> PhyRate;
    /// Translate a profile result, refusing an unsupported hardware rate.
    fn from_phy_rate(rate: PhyRate) -> Option<Self>;
}
/// The backend's EDCA queue identity.
pub trait BackendTxQueue: Copy {
    /// Standard access category of this physical queue.
    fn access_category(self) -> oer_ieee80211_wire::qos::WmmAccessCategory;
}
/// Contention effects a retry policy may apply after a detached completion.
pub trait BackendContention<Q> {
    /// Reset the queue after successful delivery.
    fn record_success(&mut self, role: crate::VifRole, queue: Q);
    /// Reset the queue after a terminal exchange.
    fn reset_terminal_exchange(&mut self, role: crate::VifRole, queue: Q);
    /// Advance the queue after a retryable failure.
    fn record_retry_failure(&mut self, role: crate::VifRole, queue: Q);
}
/// A core-owned retry state consumed through the static profile boundary.
pub trait BackendMpduRetry<R: BackendTxRate, Q: BackendTxQueue>: Sized {
    /// Start one frame's retry plan without publishing it.
    fn new(
        role: crate::VifRole,
        queue: Q,
        initial: R,
        policy: OrdinaryRetryRatePolicy,
        limit: u8,
        class: FrameClass,
    ) -> Result<Self, OrdinaryRetryError<R>>;
    /// Publications including the current attempt.
    fn publications(&self) -> u8;
    /// Failed-attempt counters.
    fn counters(&self) -> RetryCounters;
    /// Rate of the current publication.
    fn current_rate(&self) -> Result<R, OrdinaryRetryError<R>>;
    /// Inspect a later rate without advancing this state.
    fn rate_after_failed_attempts(&self, failures: u8) -> Result<R, OrdinaryRetryError<R>>;
    /// Apply one detached completion and its contention effects.
    fn observe_completion<C: BackendContention<Q>>(
        &mut self,
        contention: &mut C,
        status: crate::TxStatus,
    ) -> OrdinaryRetryDecision;
    /// Apply an observed collision.
    fn observe_collision<C: BackendContention<Q>>(
        &mut self,
        contention: &mut C,
    ) -> OrdinaryCollisionDecision;
    /// Reset contention after a non-retryable backend error.
    fn abort<C: BackendContention<Q>>(&self, contention: &mut C);
}
/// Cold AIFS and contention exponents supplied by a transmit profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DefaultContention {
    /// AIFS in slot units after SIFS.
    pub aifsn: u8,
    /// Minimum contention exponent.
    pub ecw_min: u8,
    /// Maximum contention exponent.
    pub ecw_max: u8,
}
/// A compile-time transmit profile selected by the image/runtime composition.
/// Algorithms, tables and retry-state definitions belong above the backend;
/// the backend calls this interface without a trait object or stored profile.
pub trait BackendTxProfile: Copy + core::fmt::Debug + Eq {
    /// Core-owned retry state, preserving the backend's admitted value types.
    type MpduRetry<R: BackendTxRate, Q: BackendTxQueue>: BackendMpduRetry<R, Q>;
    /// Whether ACK failures always advance the short counter.
    const ACK_FAILURE_SHORT: bool;
    /// Short retry budget of this profile.
    const SHORT_RETRY_LIMIT: u8;
    /// Long retry budget of this profile.
    const LONG_RETRY_LIMIT: u8;
    /// Default RTS length threshold in octets.
    const RTS_THRESHOLD_BYTES: u16;
    /// Cold contention in standard ACI order (BE, BK, VI, VO).
    const DEFAULT_CONTENTION: [DefaultContention; 4];
    /// Look up an exact profile-owned rate record.
    fn rate_schedule(kind: RateScheduleKind, index: u8) -> Option<RateScheduleRef>;
    /// Decode the state of a profile-owned record.
    fn schedule_state(schedule: RateScheduleRef) -> RateScheduleRecordState;
    /// Publication budget of a profile-owned record.
    fn schedule_publication_limit(schedule: RateScheduleRef) -> u8;
    /// Select a record's rate after the supplied failure count.
    fn schedule_rate_after_failures(schedule: RateScheduleRef, failures: u8) -> Option<u8>;
    /// Portable rate selected by the ordinary record walk (HT20 code domain).
    fn schedule_retry_rate(schedule: RateScheduleRef, failures: u8) -> Option<PhyRate>;
    /// Normal retry ladder of a portable rate.
    fn retry_rate(initial: PhyRate, failures: u8) -> Option<PhyRate>;
    /// Publication budget of a non-HT rate.
    fn legacy_retry_publication_limit(initial: oer_ieee80211_wire::phy::LegacyRate) -> Option<u8>;
    /// Select protection with the profile's HE duration estimate.
    fn select_protection(
        bss: crate::backend_protection::BssProtection,
        threshold: Option<crate::backend_protection::RtsLengthThreshold>,
        ppdu: crate::backend_protection::ProtectedPpdu,
    ) -> crate::backend_protection::TxProtectionDecision;
    /// Legacy-only protection without linking HE duration arithmetic.
    fn select_legacy_protection(
        bss: crate::backend_protection::BssProtection,
        threshold: Option<crate::backend_protection::RtsLengthThreshold>,
        rate: oer_ieee80211_wire::phy::LegacyRate,
        receiver: crate::backend_protection::TxReceiver,
        psdu: crate::backend_protection::TxPsdu,
    ) -> crate::backend_protection::TxProtectionDecision;
    /// Control-frame rate of a portable data rate.
    fn control_rate(
        bss: crate::backend_protection::BssProtection,
        threshold: Option<crate::backend_protection::RtsLengthThreshold>,
        rate: PhyRate,
    ) -> oer_ieee80211_wire::phy::LegacyRate;
    /// Maximum HE APEP that fits below the given protection threshold.
    fn maximum_unprotected_apep_bytes(
        threshold: crate::backend_protection::HeTxopDurationRtsThreshold,
        padding: crate::backend_protection::HePacketPadding,
        rate: oer_ieee80211_wire::phy::HeRate,
    ) -> u16;
}

impl BackendTxRate for PhyRate {
    fn phy_rate(self) -> PhyRate {
        self
    }
    fn from_phy_rate(rate: PhyRate) -> Option<Self> {
        Some(rate)
    }
}
impl BackendTxQueue for oer_ieee80211_wire::qos::WmmAccessCategory {
    fn access_category(self) -> Self {
        self
    }
}

/// Hardware inputs selected by the association rate-control profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeamformingReportRate {
    mode: u8,
    rate: u16,
    dcm: bool,
    ersu: bool,
    ersu_ack: bool,
}

impl BeamformingReportRate {
    /// Construct the complete report-rate and matching ACK request.
    pub const fn new(mode: u8, rate: u16, dcm: bool, ersu: bool, ersu_ack: bool) -> Self {
        Self {
            mode,
            rate,
            dcm,
            ersu,
            ersu_ack,
        }
    }

    /// PHY signal mode.
    pub const fn signal_mode(self) -> u8 {
        self.mode
    }

    /// Hardware rate code.
    pub const fn rate_code(self) -> u16 {
        self.rate
    }

    /// DCM selector.
    pub const fn dcm(self) -> bool {
        self.dcm
    }

    /// ER SU selector.
    pub const fn extended_range_single_user(self) -> bool {
        self.ersu
    }

    /// Matching ER SU ACK selector.
    pub const fn extended_range_ack(self) -> bool {
        self.ersu_ack
    }
}

/// Rate-control values a hardware adapter reads without knowing its core.
pub trait BackendStationRateControl {
    /// The compile-time profile owning these records.
    type Profile: BackendTxProfile;
    /// The ordinary frame's selected record.
    fn current_schedule(&self) -> RateScheduleRef;
    /// An independently selected aggregate record, when available.
    fn current_ampdu_schedule(&self) -> Option<RateScheduleRef>;
    /// Feed an observed successful ACK signal sample into the core.
    fn update_ack_snr(&mut self, sample: i8);
    /// The association's hardware beamforming report request.
    fn beamforming_report(&self) -> BeamformingReportRate;
}
