//! Rust-owned composition of ESP32-S31 `register_chipv7_phy`.
//!
//! The pinned archive parent is 486 bytes. Persistence remains outside the
//! radio driver: callers may provide and retrieve a typed calibration cache,
//! while this transition owns validation and calibration. A valid snapshot
//! seeds partial calibration; hardware-resident state is republished by the
//! ordinary cold RF/baseband graph.
//! All retained radio work is represented by owned state plus
//! one externally completed MMIO, PHY-I2C, timer or observation edge.

pub const PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT: u16 = 1_000;

const PHY_REGISTER_FINAL_I2C_ADDRESS: crate::analog::i2c::PhyI2cAddress =
    crate::analog::i2c::analog_registers::RFPLL_SDM_UPDATE_ENABLE.address();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterDelayPhase {
    ForceTxRx { enabled: bool, completed_phase: u8 },
    HardwareFrequencyDisabled,
    I2cMasterReset { index: u8, sample: u16 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterMmioAction {
    PrepareColdStart,
    ConfigureForceTxRx { enabled: bool, phase: u8 },
    ResetFrequencyModule,
    SetHardwareFrequencyControl { enabled: bool },
    PulseI2cMasterReset { index: u8 },
    ConfigureXtal40Mhz,
    SetCalibrationClock { enabled: bool },
    SetBbpllCalibration { enabled: bool },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRegisterMmioCompletion {
    pub action: PhyRegisterMmioAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterAction {
    Mmio(PhyRegisterMmioAction),
    DelayMicros {
        phase: PhyRegisterDelayPhase,
        micros: u32,
    },
    SampleI2cMasterReset {
        index: u8,
        sample: u16,
    },
    Rf(crate::analog::i2c::PhyRfInitPrefixAction),
    Baseband(crate::calibration::baseband::PhyBbInitAction),
    Temperature(crate::analog::temperature::PhyTemperatureAction),
    ReadFinalI2c {
        address: crate::analog::i2c::PhyI2cAddress,
    },
}

/// Top-level registration stage of one external action.
///
/// Run errors report this compact identity instead of the complete nested
/// action, which stays internal to the registration graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterStage {
    Mmio,
    Delay,
    I2cMasterResetSample,
    Rf,
    Baseband,
    Temperature,
    FinalI2cRead,
}

impl PhyRegisterAction {
    /// The registration stage this action belongs to.
    pub(crate) const fn stage(&self) -> PhyRegisterStage {
        match self {
            Self::Mmio(_) => PhyRegisterStage::Mmio,
            Self::DelayMicros { .. } => PhyRegisterStage::Delay,
            Self::SampleI2cMasterReset { .. } => PhyRegisterStage::I2cMasterResetSample,
            Self::Rf(_) => PhyRegisterStage::Rf,
            Self::Baseband(_) => PhyRegisterStage::Baseband,
            Self::Temperature(_) => PhyRegisterStage::Temperature,
            Self::ReadFinalI2c { .. } => PhyRegisterStage::FinalI2cRead,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterCompletion {
    Mmio(PhyRegisterMmioCompletion),
    DelayElapsed {
        phase: PhyRegisterDelayPhase,
        micros: u32,
    },
    I2cMasterResetSampled {
        index: u8,
        sample: u16,
        busy: bool,
    },
    Rf(crate::analog::i2c::PhyRfInitPrefixCompletion),
    Baseband(crate::calibration::baseband::PhyBbInitCompletion),
    Temperature(crate::analog::temperature::PhyTemperatureCompletion),
    FinalI2cRead {
        address: crate::analog::i2c::PhyI2cAddress,
        value: u8,
    },
    FinalI2cDeadlineExceeded {
        address: crate::analog::i2c::PhyI2cAddress,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterFailure {
    I2cMasterResetTimedOut { index: u8, samples: u16 },
    Rf(crate::analog::i2c::PhyRfInitPrefixOutcome),
    Baseband(crate::calibration::baseband::PhyBbInitFailure),
    Temperature(crate::analog::temperature::PhyTemperatureFailure),
    FinalI2cDeadlineExceeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationIdentity {
    pub rf_cal_version: u32,
    pub base_mac_address: [u8; 6],
    pub mac_extension: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationPath {
    /// No caller-owned persistence was requested.
    FullUncached,
    /// No retained cache was supplied; a fresh cache is produced.
    FullForCache,
    /// A supplied cache could not be safely replayed; full calibration
    /// replaces it.
    FullAfterRejectedCache,
    /// A valid cache seeded semantic calibration state; cold RF/baseband
    /// initialization republishes hardware-resident products and performs the
    /// remaining partial-calibration work.
    PartialFromCache,
}

impl PhyCalibrationPath {
    pub const fn cache_available(self) -> bool {
        !matches!(self, Self::FullUncached)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRegisterOutcome {
    /// Observed RF calibration results; completion alone does not imply PLL lock.
    #[cfg(feature = "registration-diagnostics")]
    pub rf_calibration: Option<RfCalibrationDiagnostics>,
    pub full_calibration_performed: bool,
    pub calibration_path: PhyCalibrationPath,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterLocalStep {
    StateAdvanced,
    External(PhyRegisterAction),
    Complete(PhyRegisterOutcome),
    Failed(PhyRegisterFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterTransitionError {
    WrongCompletion,
    AlreadyComplete,
    MissingStateOwner,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreludeStep {
    Prepare,
    ForceFirst,
    ForceFirstDelay,
    ForceSecond,
    ForceSecondDelay,
    FrequencyReset,
    DisableHardwareFrequency,
    DisableHardwareFrequencyDelay,
    I2cInitialSample { index: u8 },
    I2cResetPulse { index: u8 },
    I2cResetDelay { index: u8, sample: u16 },
    I2cResetSample { index: u8, sample: u16 },
    ApplyProfile,
    ConfigureXtal,
    CalibrationClockOn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TailStep {
    CalibrationClockOff,
    Temperature,
    BackupCalibration,
    BbpllOff,
    FinalI2cRead,
    MarkRegistered,
    EnableHardwareFrequency,
    ReleaseFirst,
    ReleaseFirstDelay,
    ReleaseSecond,
    ReleaseSecondDelay,
}

#[allow(
    clippy::large_enum_variant,
    reason = "the heap-free registration owner retains exactly one complete calibration phase"
)]
enum Phase {
    Prelude(PreludeStep),
    Rf(crate::calibration::cold::PhyRfColdInit),
    Baseband(crate::calibration::baseband::PhyBbInitTransition),
    Temperature(crate::analog::temperature::PhyTemperatureTransition),
    Tail(TailStep),
    Cleanup {
        step: TailStep,
        failure: PhyRegisterFailure,
    },
    Complete(PhyRegisterOutcome),
    Failed(PhyRegisterFailure),
}

/// Unique proof that the source-owned target PHY registration runner completed.
///
/// This owner is intentionally neither `Copy` nor `Clone`, and its constructor
/// requires a private target witness. A generic model transition, including one
/// driven entirely by synthetic safe completions, can therefore recover only an
/// ordinary [`crate::state::PhyState`] and cannot promote it to registered
/// target state.
///
/// The wrapper deliberately exposes no mutable reference to the inner state.
/// Such a reference would allow safe code to replace the calibrated state
/// while retaining this proof. The proof also has no public decomposer: role
/// owners must retain the target-issued radio/state association or explicitly
/// use an API which consumes and discards the proof.
///
/// This token records completion of the target execution path. It is not, by
/// itself, RF qualification, link evidence, or a claim of operational IEEE
/// 802.15.4 readiness.
///
/// ```compile_fail
/// use oer_esp32s31_phy::RegisteredPhyState;
///
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<RegisteredPhyState>();
/// ```
///
/// ```compile_fail
/// use oer_esp32s31_phy::{PhyConfig, PhyState, RegisteredPhyState};
///
/// let ordinary = PhyState::new(PhyConfig::production());
/// let _forged = RegisteredPhyState { state: ordinary };
/// ```
///
/// ```compile_fail
/// use oer_esp32s31_phy::{PhyConfig, PhyState, RegisteredPhyState};
///
/// fn replace_state(registered: &mut RegisteredPhyState) {
///     let ordinary = PhyState::new(PhyConfig::production());
///     let _old = core::mem::replace(registered.state_mut(), ordinary);
/// }
/// ```
///
/// ```compile_fail
/// use oer_esp32s31_phy::PhyRegisterTransition;
///
/// let model = PhyRegisterTransition::with_production_config();
/// let _hardware_proof = model.into_registered_parts();
/// ```
///
/// ```compile_fail
/// use oer_esp32s31_phy::{PhyRegisterTransition, RegisteredPhyState};
///
/// let model = PhyRegisterTransition::with_production_config();
/// if let Ok((ordinary, _cache)) = model.into_model_parts() {
///     let _hardware_proof: RegisteredPhyState = ordinary;
/// }
/// ```
///
/// ```compile_fail
/// use oer_esp32s31_phy::RegisteredPhyState;
///
/// fn discard_proof(registered: RegisteredPhyState) {
///     let _ordinary = registered.into_state();
/// }
/// ```
pub struct RegisteredPhyState {
    state: crate::state::PhyState,
}

impl RegisteredPhyState {
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn from_target_completion(
        state: crate::state::PhyState,
        _witness: crate::target_port::TargetRegistrationWitness,
    ) -> Self {
        Self { state }
    }

    /// Borrow the calibrated state without weakening the registration proof.
    pub const fn state(&self) -> &crate::state::PhyState {
        &self.state
    }

    /// Project periodic-tracking policy from the one registered PHY epoch.
    ///
    /// Keeping this method on the registration proof prevents callers from
    /// selecting cold-image predicates independently of the state they govern.
    pub(crate) const fn tracking_policy(
        &self,
    ) -> crate::tracking::parameters::PhyParamTrackingPolicy {
        crate::tracking::parameters::PhyParamTrackingPolicy::for_registered_state(&self.state)
    }

    /// Borrow the live semantic state only inside a target operation which
    /// already retains the matching registered-radio epoch.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn target_state_mut(&mut self) -> &mut crate::state::PhyState {
        &mut self.state
    }

    /// Build an internal wrapper fixture without adding a production mint path.
    #[cfg(test)]
    pub(crate) fn from_wrapper_test_model(mut state: crate::state::PhyState) -> Self {
        state.mark_phy_registered();
        Self { state }
    }
}

/// Complete full-calibration state machine replacing the stateful vendor
/// parent. Only one phase owns `PhyState` at a time: the outer transition,
/// `PhyRfColdInit`, or `PhyBbInitTransition`.
pub struct PhyRegisterTransition {
    state: Option<crate::state::PhyState>,
    config: Option<crate::state::PhyConfig>,
    channel_or_frequency: u16,
    calibration_identity: Option<PhyCalibrationIdentity>,
    calibration_cache: Option<crate::state::PhyCalibrationCache>,
    calibration_candidate: bool,
    calibration_cache_ready: bool,
    calibration_path: PhyCalibrationPath,
    temperature_control: Option<crate::calibration::cold::PhyRegisterTemperatureControl>,
    #[cfg(feature = "registration-diagnostics")]
    rf_calibration: Option<RfCalibrationDiagnostics>,
    phase: Option<Phase>,
}

impl PhyRegisterTransition {
    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn new(config: crate::state::PhyConfig) -> Self {
        Self::new_on_channel(config, 11)
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn new_on_channel(
        config: crate::state::PhyConfig,
        channel_or_frequency: u16,
    ) -> Self {
        Self {
            state: Some(crate::state::PhyState::new(
                crate::state::PhyConfig::esp32s31_default(),
            )),
            config: Some(config),
            channel_or_frequency,
            calibration_identity: None,
            calibration_cache: None,
            calibration_candidate: false,
            calibration_cache_ready: false,
            calibration_path: PhyCalibrationPath::FullUncached,
            temperature_control: None,
            #[cfg(feature = "registration-diagnostics")]
            rf_calibration: None,
            phase: Some(Phase::Prelude(PreludeStep::Prepare)),
        }
    }

    /// Construct a cold transition with caller-owned calibration persistence.
    ///
    /// `identity` is obtained by the platform layer (RF-calibration ABI
    /// version plus the two eFuse identity words). `cache` may come from any
    /// caller-selected backend. This type never reads eFuse, NVS or flash.
    pub const fn new_on_channel_with_calibration(
        config: crate::state::PhyConfig,
        channel_or_frequency: u16,
        identity: PhyCalibrationIdentity,
        cache: Option<crate::state::PhyCalibrationCache>,
    ) -> Self {
        let calibration_candidate = cache.is_some();
        Self {
            state: Some(crate::state::PhyState::new(
                crate::state::PhyConfig::esp32s31_default(),
            )),
            config: Some(config),
            channel_or_frequency,
            calibration_identity: Some(identity),
            calibration_cache: cache,
            calibration_candidate,
            calibration_cache_ready: false,
            calibration_path: if calibration_candidate {
                PhyCalibrationPath::FullAfterRejectedCache
            } else {
                PhyCalibrationPath::FullForCache
            },
            temperature_control: None,
            #[cfg(feature = "registration-diagnostics")]
            rf_calibration: None,
            phase: Some(Phase::Prelude(PreludeStep::Prepare)),
        }
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn with_production_config() -> Self {
        Self::new(crate::state::PhyConfig::production())
    }

    #[cfg(feature = "validation-probes")]
    pub const fn with_production_config_on_channel(channel_or_frequency: u16) -> Self {
        Self::new_on_channel(crate::state::PhyConfig::production(), channel_or_frequency)
    }

    pub const fn with_production_config_and_calibration(
        identity: PhyCalibrationIdentity,
        cache: Option<crate::state::PhyCalibrationCache>,
    ) -> Self {
        Self::new_on_channel_with_calibration(
            crate::state::PhyConfig::production(),
            11,
            identity,
            cache,
        )
    }

    #[cfg(feature = "validation-probes")]
    pub const fn with_production_config_on_channel_and_calibration(
        channel_or_frequency: u16,
        identity: PhyCalibrationIdentity,
        cache: Option<crate::state::PhyCalibrationCache>,
    ) -> Self {
        Self::new_on_channel_with_calibration(
            crate::state::PhyConfig::production(),
            channel_or_frequency,
            identity,
            cache,
        )
    }

    /// Return the freshly completed full- or partial-calibration cache.
    ///
    /// Failed and in-progress transitions never expose a cache as persistable.
    #[cfg(any(test, feature = "validation-probes"))]
    pub fn calibration_cache(&self) -> Option<&crate::state::PhyCalibrationCache> {
        (self.calibration_cache_ready && matches!(self.phase.as_ref(), Some(Phase::Complete(_))))
            .then_some(())
            .and(self.calibration_cache.as_ref())
    }

    pub fn state(&self) -> Option<&crate::state::PhyState> {
        self.state.as_ref().or_else(|| match self.phase.as_ref()? {
            Phase::Rf(transition) => Some(transition.state()),
            Phase::Baseband(transition) => Some(transition.state()),
            _ => None,
        })
    }

    /// Recover the ordinary state and cache from a completed model transition.
    ///
    /// Extraction is available only after the transition reaches
    /// [`PhyRegisterLocalStep::Complete`] and retains its registered state
    /// marker. This API accepts a caller-provided completion oracle, so its
    /// result is deliberately an ordinary [`crate::state::PhyState`], not
    /// [`RegisteredPhyState`]. Synthetic safe completions can exercise the host
    /// model but can never manufacture target-registration authority.
    ///
    /// Every non-success path returns the complete transition without moving
    /// either owner out of it. Target code must use the opaque target attempt
    /// and runner exported on RISC-V instead of this model extractor.
    #[allow(
        clippy::result_large_err,
        reason = "failure must return the unique allocation-free registration owner"
    )]
    pub fn into_model_parts(
        mut self,
    ) -> Result<
        (
            crate::state::PhyState,
            Option<crate::state::PhyCalibrationCache>,
        ),
        Self,
    > {
        if !matches!(self.phase.as_ref(), Some(Phase::Complete(_))) {
            return Err(self);
        }
        if !self
            .state
            .as_ref()
            .is_some_and(crate::state::PhyState::phy_registered)
        {
            return Err(self);
        }
        let Some(state) = self.state.take() else {
            return Err(self);
        };
        let calibration_cache = if self.calibration_cache_ready {
            self.calibration_cache.take()
        } else {
            None
        };
        Ok((state, calibration_cache))
    }

    /// Whether failure cleanup reached its terminal phase with the state owner
    /// intact. This is a model fact, not independent proof of target MMIO; target
    /// runners may expose it only for their own executed transition.
    pub fn failure_cleanup_completed(&self) -> bool {
        matches!(self.phase.as_ref(), Some(Phase::Failed(_))) && self.state.is_some()
    }

    /// Recover the ordinary PHY owner and retry cache after terminal failure.
    ///
    /// Success and in-progress transitions remain owned by the returned
    /// transition, so this path can never manufacture [`RegisteredPhyState`].
    /// The optional cache is returned only to preserve caller ownership across
    /// a retry. It is not a persistable output of a successful registration.
    #[allow(
        clippy::result_large_err,
        reason = "non-failure paths must return the unique allocation-free registration owner"
    )]
    #[cfg(any(test, feature = "validation-probes"))]
    pub fn into_failed_parts(
        mut self,
    ) -> Result<
        (
            crate::state::PhyState,
            Option<crate::state::PhyCalibrationCache>,
        ),
        Self,
    > {
        if !self.failure_cleanup_completed() {
            return Err(self);
        }
        let Some(state) = self.state.take() else {
            return Err(self);
        };
        Ok((state, self.calibration_cache.take()))
    }

    fn begin_cleanup(&mut self, failure: PhyRegisterFailure) {
        self.calibration_cache_ready = false;
        self.phase = Some(Phase::Cleanup {
            step: TailStep::CalibrationClockOff,
            failure,
        });
    }

    fn tail_action(step: TailStep) -> Option<PhyRegisterAction> {
        match step {
            TailStep::CalibrationClockOff => Some(PhyRegisterAction::Mmio(
                PhyRegisterMmioAction::SetCalibrationClock { enabled: false },
            )),
            TailStep::BbpllOff => Some(PhyRegisterAction::Mmio(
                PhyRegisterMmioAction::SetBbpllCalibration { enabled: false },
            )),
            TailStep::FinalI2cRead => Some(PhyRegisterAction::ReadFinalI2c {
                address: PHY_REGISTER_FINAL_I2C_ADDRESS,
            }),
            TailStep::EnableHardwareFrequency => Some(PhyRegisterAction::Mmio(
                PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: true },
            )),
            TailStep::ReleaseFirst => Some(PhyRegisterAction::Mmio(
                PhyRegisterMmioAction::ConfigureForceTxRx {
                    enabled: false,
                    phase: 0,
                },
            )),
            TailStep::ReleaseFirstDelay => Some(PhyRegisterAction::DelayMicros {
                phase: PhyRegisterDelayPhase::ForceTxRx {
                    enabled: false,
                    completed_phase: 0,
                },
                micros: 1,
            }),
            TailStep::ReleaseSecond => Some(PhyRegisterAction::Mmio(
                PhyRegisterMmioAction::ConfigureForceTxRx {
                    enabled: false,
                    phase: 1,
                },
            )),
            TailStep::ReleaseSecondDelay => Some(PhyRegisterAction::DelayMicros {
                phase: PhyRegisterDelayPhase::ForceTxRx {
                    enabled: false,
                    completed_phase: 1,
                },
                micros: 1,
            }),
            TailStep::Temperature | TailStep::BackupCalibration | TailStep::MarkRegistered => None,
        }
    }

    pub fn step_local(&mut self) -> Result<PhyRegisterLocalStep, PhyRegisterTransitionError> {
        let phase = self
            .phase
            .take()
            .ok_or(PhyRegisterTransitionError::AlreadyComplete)?;
        match phase {
            Phase::Prelude(PreludeStep::ApplyProfile) => {
                let Some(config) = self.config.take() else {
                    self.phase = Some(Phase::Prelude(PreludeStep::ApplyProfile));
                    return Err(PhyRegisterTransitionError::MissingStateOwner);
                };
                let Some(state) = self.state.as_mut() else {
                    self.config = Some(config);
                    self.phase = Some(Phase::Prelude(PreludeStep::ApplyProfile));
                    return Err(PhyRegisterTransitionError::MissingStateOwner);
                };
                self.calibration_cache_ready = false;
                let restored = match (self.calibration_identity, self.calibration_cache.as_ref()) {
                    (Some(identity), Some(cache)) => state
                        .begin_cached_calibration(config, cache, identity)
                        .is_ok(),
                    _ => false,
                };
                if restored {
                    self.calibration_path = PhyCalibrationPath::PartialFromCache;
                } else {
                    if self.calibration_candidate {
                        self.calibration_path = PhyCalibrationPath::FullAfterRejectedCache;
                    }
                    state.begin_full_calibration(config);
                }
                // Pinned parent saves this flag word before either child can
                // mutate it and uses the snapshot after both return.
                self.temperature_control = Some(state.register_temperature_control());
                self.phase = Some(Phase::Prelude(PreludeStep::ConfigureXtal));
                Ok(PhyRegisterLocalStep::StateAdvanced)
            }
            Phase::Prelude(step) => {
                let action = match step {
                    PreludeStep::Prepare => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::PrepareColdStart)
                    }
                    PreludeStep::ForceFirst => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::ConfigureForceTxRx {
                            enabled: true,
                            phase: 0,
                        })
                    }
                    PreludeStep::ForceFirstDelay => PhyRegisterAction::DelayMicros {
                        phase: PhyRegisterDelayPhase::ForceTxRx {
                            enabled: true,
                            completed_phase: 0,
                        },
                        micros: 1,
                    },
                    PreludeStep::ForceSecond => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::ConfigureForceTxRx {
                            enabled: true,
                            phase: 1,
                        })
                    }
                    PreludeStep::ForceSecondDelay => PhyRegisterAction::DelayMicros {
                        phase: PhyRegisterDelayPhase::ForceTxRx {
                            enabled: true,
                            completed_phase: 1,
                        },
                        micros: 1,
                    },
                    PreludeStep::FrequencyReset => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::ResetFrequencyModule)
                    }
                    PreludeStep::DisableHardwareFrequency => PhyRegisterAction::Mmio(
                        PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: false },
                    ),
                    PreludeStep::DisableHardwareFrequencyDelay => PhyRegisterAction::DelayMicros {
                        phase: PhyRegisterDelayPhase::HardwareFrequencyDisabled,
                        micros: 2,
                    },
                    PreludeStep::I2cInitialSample { index }
                    | PreludeStep::I2cResetSample { index, sample: 0 } => {
                        PhyRegisterAction::SampleI2cMasterReset { index, sample: 0 }
                    }
                    PreludeStep::I2cResetSample { index, sample } => {
                        PhyRegisterAction::SampleI2cMasterReset { index, sample }
                    }
                    PreludeStep::I2cResetPulse { index } => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::PulseI2cMasterReset {
                            index,
                        })
                    }
                    PreludeStep::I2cResetDelay { index, sample } => {
                        PhyRegisterAction::DelayMicros {
                            phase: PhyRegisterDelayPhase::I2cMasterReset { index, sample },
                            micros: 1,
                        }
                    }
                    PreludeStep::ConfigureXtal => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::ConfigureXtal40Mhz)
                    }
                    PreludeStep::CalibrationClockOn => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::SetCalibrationClock {
                            enabled: true,
                        })
                    }
                    PreludeStep::ApplyProfile => unreachable!(),
                };
                self.phase = Some(Phase::Prelude(step));
                Ok(PhyRegisterLocalStep::External(action))
            }
            Phase::Rf(mut transition) => {
                let local = match transition.step_local() {
                    Ok(local) => local,
                    Err(error) => {
                        self.phase = Some(Phase::Rf(transition));
                        return Err(error.into());
                    }
                };
                match local {
                    crate::calibration::cold::PhyColdLocalStep::StateAdvanced => {
                        self.phase = Some(Phase::Rf(transition));
                        Ok(PhyRegisterLocalStep::StateAdvanced)
                    }
                    crate::calibration::cold::PhyColdLocalStep::External(action) => {
                        self.phase = Some(Phase::Rf(transition));
                        Ok(PhyRegisterLocalStep::External(PhyRegisterAction::Rf(
                            action,
                        )))
                    }
                    crate::calibration::cold::PhyColdLocalStep::Complete(outcome) => {
                        let successful = matches!(
                            outcome,
                            crate::analog::i2c::PhyRfInitPrefixOutcome::ChannelFrequencyInitialized { .. }
                        );
                        #[cfg(feature = "registration-diagnostics")]
                        {
                            self.rf_calibration = RfCalibrationDiagnostics::from_outcome(outcome);
                        }
                        let state = transition.into_state();
                        if successful {
                            self.phase = Some(Phase::Baseband(
                                crate::calibration::baseband::PhyBbInitTransition::new_on_channel(
                                    state,
                                    self.channel_or_frequency,
                                ),
                            ));
                        } else {
                            self.state = Some(state);
                            self.begin_cleanup(PhyRegisterFailure::Rf(outcome));
                        }
                        Ok(PhyRegisterLocalStep::StateAdvanced)
                    }
                }
            }
            Phase::Baseband(mut transition) => {
                let local = match transition.step_local() {
                    Ok(local) => local,
                    Err(error) => {
                        self.phase = Some(Phase::Baseband(transition));
                        return Err(error.into());
                    }
                };
                match local {
                    crate::calibration::baseband::PhyBbInitLocalStep::StateAdvanced => {
                        self.phase = Some(Phase::Baseband(transition));
                        Ok(PhyRegisterLocalStep::StateAdvanced)
                    }
                    crate::calibration::baseband::PhyBbInitLocalStep::External(action) => {
                        self.phase = Some(Phase::Baseband(transition));
                        Ok(PhyRegisterLocalStep::External(PhyRegisterAction::Baseband(
                            action,
                        )))
                    }
                    crate::calibration::baseband::PhyBbInitLocalStep::Complete(_) => {
                        self.state = Some(transition.into_state());
                        self.phase = Some(Phase::Tail(TailStep::CalibrationClockOff));
                        Ok(PhyRegisterLocalStep::StateAdvanced)
                    }
                    crate::calibration::baseband::PhyBbInitLocalStep::Failed(failure) => {
                        self.state = Some(transition.into_state());
                        self.begin_cleanup(PhyRegisterFailure::Baseband(failure));
                        Ok(PhyRegisterLocalStep::StateAdvanced)
                    }
                }
            }
            Phase::Temperature(transition) => match transition.action() {
                crate::analog::temperature::PhyTemperatureAction::Complete(outcome) => {
                    let Some(control) = self.temperature_control else {
                        self.phase = Some(Phase::Temperature(transition));
                        return Err(PhyRegisterTransitionError::MissingStateOwner);
                    };
                    let Some(state) = self.state.as_mut() else {
                        self.phase = Some(Phase::Temperature(transition));
                        return Err(PhyRegisterTransitionError::MissingStateOwner);
                    };
                    state.apply_register_temperature_outcome(control, outcome);
                    // Cache replay is intentionally not exposed until the
                    // driver owns restoration of every retained RF/baseband
                    // hardware value. Every supported path therefore backs up
                    // the fresh full-calibration result.
                    self.phase = Some(Phase::Tail(TailStep::BackupCalibration));
                    Ok(PhyRegisterLocalStep::StateAdvanced)
                }
                crate::analog::temperature::PhyTemperatureAction::Failed(failure) => {
                    self.begin_cleanup(PhyRegisterFailure::Temperature(failure));
                    Ok(PhyRegisterLocalStep::StateAdvanced)
                }
                action => {
                    self.phase = Some(Phase::Temperature(transition));
                    Ok(PhyRegisterLocalStep::External(
                        PhyRegisterAction::Temperature(action),
                    ))
                }
            },
            Phase::Tail(TailStep::Temperature) => {
                self.phase = Some(Phase::Temperature(
                    crate::analog::temperature::PhyTemperatureTransition::new(),
                ));
                Ok(PhyRegisterLocalStep::StateAdvanced)
            }
            Phase::Tail(TailStep::BackupCalibration) => {
                // Keep the caller's cache intact until the final release edge.
                // The semantic calibration state does not change in the
                // remaining tail; terminal success captures its replacement
                // atomically immediately before entering `Complete`.
                self.phase = Some(Phase::Tail(TailStep::BbpllOff));
                Ok(PhyRegisterLocalStep::StateAdvanced)
            }
            Phase::Tail(TailStep::MarkRegistered) => {
                let Some(state) = self.state.as_mut() else {
                    self.phase = Some(Phase::Tail(TailStep::MarkRegistered));
                    return Err(PhyRegisterTransitionError::MissingStateOwner);
                };
                state.mark_phy_registered();
                self.phase = Some(Phase::Tail(TailStep::EnableHardwareFrequency));
                Ok(PhyRegisterLocalStep::StateAdvanced)
            }
            Phase::Tail(step) => {
                let action =
                    Self::tail_action(step).ok_or(PhyRegisterTransitionError::WrongCompletion)?;
                self.phase = Some(Phase::Tail(step));
                Ok(PhyRegisterLocalStep::External(action))
            }
            Phase::Cleanup {
                step:
                    TailStep::Temperature
                    | TailStep::BackupCalibration
                    | TailStep::FinalI2cRead
                    | TailStep::MarkRegistered,
                failure,
            } => {
                self.phase = Some(Phase::Cleanup {
                    step: TailStep::BbpllOff,
                    failure,
                });
                Ok(PhyRegisterLocalStep::StateAdvanced)
            }
            Phase::Cleanup { step, failure } => {
                let action =
                    Self::tail_action(step).ok_or(PhyRegisterTransitionError::WrongCompletion)?;
                self.phase = Some(Phase::Cleanup { step, failure });
                Ok(PhyRegisterLocalStep::External(action))
            }
            Phase::Complete(outcome) => {
                self.phase = Some(Phase::Complete(outcome));
                Ok(PhyRegisterLocalStep::Complete(outcome))
            }
            Phase::Failed(failure) => {
                self.phase = Some(Phase::Failed(failure));
                Ok(PhyRegisterLocalStep::Failed(failure))
            }
        }
    }

    pub fn advance_external(
        &mut self,
        completion: PhyRegisterCompletion,
    ) -> Result<(), PhyRegisterTransitionError> {
        let phase = self
            .phase
            .take()
            .ok_or(PhyRegisterTransitionError::AlreadyComplete)?;
        let next = match (phase, completion) {
            (Phase::Prelude(PreludeStep::Prepare), PhyRegisterCompletion::Mmio(completed))
                if completed.action == PhyRegisterMmioAction::PrepareColdStart =>
            {
                Phase::Prelude(PreludeStep::ForceFirst)
            }
            (Phase::Prelude(PreludeStep::ForceFirst), PhyRegisterCompletion::Mmio(completed))
                if completed.action
                    == (PhyRegisterMmioAction::ConfigureForceTxRx {
                        enabled: true,
                        phase: 0,
                    }) =>
            {
                Phase::Prelude(PreludeStep::ForceFirstDelay)
            }
            (
                Phase::Prelude(PreludeStep::ForceFirstDelay),
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::ForceTxRx {
                            enabled: true,
                            completed_phase: 0,
                        },
                    micros: 1,
                },
            ) => Phase::Prelude(PreludeStep::ForceSecond),
            (Phase::Prelude(PreludeStep::ForceSecond), PhyRegisterCompletion::Mmio(completed))
                if completed.action
                    == (PhyRegisterMmioAction::ConfigureForceTxRx {
                        enabled: true,
                        phase: 1,
                    }) =>
            {
                Phase::Prelude(PreludeStep::ForceSecondDelay)
            }
            (
                Phase::Prelude(PreludeStep::ForceSecondDelay),
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::ForceTxRx {
                            enabled: true,
                            completed_phase: 1,
                        },
                    micros: 1,
                },
            ) => Phase::Prelude(PreludeStep::FrequencyReset),
            (
                Phase::Prelude(PreludeStep::FrequencyReset),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action == PhyRegisterMmioAction::ResetFrequencyModule => {
                Phase::Prelude(PreludeStep::DisableHardwareFrequency)
            }
            (
                Phase::Prelude(PreludeStep::DisableHardwareFrequency),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: false }) =>
            {
                Phase::Prelude(PreludeStep::DisableHardwareFrequencyDelay)
            }
            (
                Phase::Prelude(PreludeStep::DisableHardwareFrequencyDelay),
                PhyRegisterCompletion::DelayElapsed {
                    phase: PhyRegisterDelayPhase::HardwareFrequencyDisabled,
                    micros: 2,
                },
            ) => Phase::Prelude(PreludeStep::I2cInitialSample { index: 0 }),
            (
                Phase::Prelude(PreludeStep::I2cInitialSample { index }),
                PhyRegisterCompletion::I2cMasterResetSampled {
                    index: completed_index,
                    sample: 0,
                    busy,
                },
            ) if completed_index == index => {
                if busy {
                    Phase::Prelude(PreludeStep::I2cResetPulse { index })
                } else if index == 0 {
                    Phase::Prelude(PreludeStep::I2cInitialSample { index: 1 })
                } else {
                    Phase::Prelude(PreludeStep::ApplyProfile)
                }
            }
            (
                Phase::Prelude(PreludeStep::I2cResetPulse { index }),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action == PhyRegisterMmioAction::PulseI2cMasterReset { index } => {
                Phase::Prelude(PreludeStep::I2cResetDelay { index, sample: 0 })
            }
            (
                Phase::Prelude(PreludeStep::I2cResetDelay { index, sample }),
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::I2cMasterReset {
                            index: completed_index,
                            sample: completed_sample,
                        },
                    micros: 1,
                },
            ) if completed_index == index && completed_sample == sample => {
                Phase::Prelude(PreludeStep::I2cResetSample { index, sample })
            }
            (
                Phase::Prelude(PreludeStep::I2cResetSample { index, sample }),
                PhyRegisterCompletion::I2cMasterResetSampled {
                    index: completed_index,
                    sample: completed_sample,
                    busy,
                },
            ) if completed_index == index && completed_sample == sample => {
                if !busy {
                    if index == 0 {
                        Phase::Prelude(PreludeStep::I2cInitialSample { index: 1 })
                    } else {
                        Phase::Prelude(PreludeStep::ApplyProfile)
                    }
                } else if sample + 1 == PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT {
                    self.begin_cleanup(PhyRegisterFailure::I2cMasterResetTimedOut {
                        index,
                        samples: sample + 1,
                    });
                    return Ok(());
                } else {
                    Phase::Prelude(PreludeStep::I2cResetDelay {
                        index,
                        sample: sample + 1,
                    })
                }
            }
            (
                Phase::Prelude(PreludeStep::ConfigureXtal),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action == PhyRegisterMmioAction::ConfigureXtal40Mhz => {
                Phase::Prelude(PreludeStep::CalibrationClockOn)
            }
            (
                Phase::Prelude(PreludeStep::CalibrationClockOn),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::SetCalibrationClock { enabled: true }) =>
            {
                let Some(state) = self.state.take() else {
                    self.phase = Some(Phase::Prelude(PreludeStep::CalibrationClockOn));
                    return Err(PhyRegisterTransitionError::MissingStateOwner);
                };
                Phase::Rf(crate::calibration::cold::PhyRfColdInit::new(state))
            }
            (Phase::Rf(mut transition), PhyRegisterCompletion::Rf(completed)) => {
                if let Err(error) = transition.advance_external(completed) {
                    self.phase = Some(Phase::Rf(transition));
                    return Err(error.into());
                }
                Phase::Rf(transition)
            }
            (Phase::Baseband(mut transition), PhyRegisterCompletion::Baseband(completed)) => {
                if let Err(error) = transition.advance_external(completed) {
                    self.phase = Some(Phase::Baseband(transition));
                    return Err(error.into());
                }
                Phase::Baseband(transition)
            }
            (
                Phase::Tail(TailStep::CalibrationClockOff),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::SetCalibrationClock { enabled: false }) =>
            {
                Phase::Tail(TailStep::Temperature)
            }
            (Phase::Temperature(mut transition), PhyRegisterCompletion::Temperature(completed)) => {
                if transition.advance(completed).is_err() {
                    self.phase = Some(Phase::Temperature(transition));
                    return Err(PhyRegisterTransitionError::WrongCompletion);
                }
                Phase::Temperature(transition)
            }
            (Phase::Tail(TailStep::BbpllOff), PhyRegisterCompletion::Mmio(completed))
                if completed.action
                    == (PhyRegisterMmioAction::SetBbpllCalibration { enabled: false }) =>
            {
                Phase::Tail(TailStep::FinalI2cRead)
            }
            (
                Phase::Tail(TailStep::FinalI2cRead),
                PhyRegisterCompletion::FinalI2cRead { address, .. },
            ) if address == PHY_REGISTER_FINAL_I2C_ADDRESS => Phase::Tail(TailStep::MarkRegistered),
            (
                Phase::Tail(TailStep::FinalI2cRead),
                PhyRegisterCompletion::FinalI2cDeadlineExceeded { address },
            ) if address == PHY_REGISTER_FINAL_I2C_ADDRESS => {
                self.begin_cleanup(PhyRegisterFailure::FinalI2cDeadlineExceeded);
                return Ok(());
            }
            (
                Phase::Tail(TailStep::EnableHardwareFrequency),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: true }) =>
            {
                Phase::Tail(TailStep::ReleaseFirst)
            }
            (Phase::Tail(TailStep::ReleaseFirst), PhyRegisterCompletion::Mmio(completed))
                if completed.action
                    == (PhyRegisterMmioAction::ConfigureForceTxRx {
                        enabled: false,
                        phase: 0,
                    }) =>
            {
                Phase::Tail(TailStep::ReleaseFirstDelay)
            }
            (
                Phase::Tail(TailStep::ReleaseFirstDelay),
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::ForceTxRx {
                            enabled: false,
                            completed_phase: 0,
                        },
                    micros: 1,
                },
            ) => Phase::Tail(TailStep::ReleaseSecond),
            (Phase::Tail(TailStep::ReleaseSecond), PhyRegisterCompletion::Mmio(completed))
                if completed.action
                    == (PhyRegisterMmioAction::ConfigureForceTxRx {
                        enabled: false,
                        phase: 1,
                    }) =>
            {
                Phase::Tail(TailStep::ReleaseSecondDelay)
            }
            (
                Phase::Tail(TailStep::ReleaseSecondDelay),
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::ForceTxRx {
                            enabled: false,
                            completed_phase: 1,
                        },
                    micros: 1,
                },
            ) => {
                if let Some(identity) = self.calibration_identity {
                    let Some(state) = self.state.as_ref() else {
                        self.phase = Some(Phase::Tail(TailStep::ReleaseSecondDelay));
                        return Err(PhyRegisterTransitionError::MissingStateOwner);
                    };
                    self.calibration_cache = Some(state.calibration_cache(identity));
                    self.calibration_cache_ready = true;
                }
                Phase::Complete(PhyRegisterOutcome {
                    #[cfg(feature = "registration-diagnostics")]
                    rf_calibration: self.rf_calibration,
                    full_calibration_performed: !matches!(
                        self.calibration_path,
                        PhyCalibrationPath::PartialFromCache
                    ),
                    calibration_path: self.calibration_path,
                })
            }
            (
                Phase::Cleanup {
                    step: TailStep::CalibrationClockOff,
                    failure,
                },
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::SetCalibrationClock { enabled: false }) =>
            {
                Phase::Cleanup {
                    step: TailStep::BbpllOff,
                    failure,
                }
            }
            (
                Phase::Cleanup {
                    step: TailStep::BbpllOff,
                    failure,
                },
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::SetBbpllCalibration { enabled: false }) =>
            {
                Phase::Cleanup {
                    step: TailStep::EnableHardwareFrequency,
                    failure,
                }
            }
            (
                Phase::Cleanup {
                    step: TailStep::EnableHardwareFrequency,
                    failure,
                },
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled: true }) =>
            {
                Phase::Cleanup {
                    step: TailStep::ReleaseFirst,
                    failure,
                }
            }
            (
                Phase::Cleanup {
                    step: TailStep::ReleaseFirst,
                    failure,
                },
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::ConfigureForceTxRx {
                    enabled: false,
                    phase: 0,
                }) =>
            {
                Phase::Cleanup {
                    step: TailStep::ReleaseFirstDelay,
                    failure,
                }
            }
            (
                Phase::Cleanup {
                    step: TailStep::ReleaseFirstDelay,
                    failure,
                },
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::ForceTxRx {
                            enabled: false,
                            completed_phase: 0,
                        },
                    micros: 1,
                },
            ) => Phase::Cleanup {
                step: TailStep::ReleaseSecond,
                failure,
            },
            (
                Phase::Cleanup {
                    step: TailStep::ReleaseSecond,
                    failure,
                },
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action
                == (PhyRegisterMmioAction::ConfigureForceTxRx {
                    enabled: false,
                    phase: 1,
                }) =>
            {
                Phase::Cleanup {
                    step: TailStep::ReleaseSecondDelay,
                    failure,
                }
            }
            (
                Phase::Cleanup {
                    step: TailStep::ReleaseSecondDelay,
                    failure,
                },
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::ForceTxRx {
                            enabled: false,
                            completed_phase: 1,
                        },
                    micros: 1,
                },
            ) => Phase::Failed(failure),
            (Phase::Complete(outcome), _) => {
                self.phase = Some(Phase::Complete(outcome));
                return Err(PhyRegisterTransitionError::AlreadyComplete);
            }
            (Phase::Failed(failure), _) => {
                self.phase = Some(Phase::Failed(failure));
                return Err(PhyRegisterTransitionError::AlreadyComplete);
            }
            (phase, _) => {
                self.phase = Some(phase);
                return Err(PhyRegisterTransitionError::WrongCompletion);
            }
        };
        self.phase = Some(next);
        Ok(())
    }
}

impl From<crate::calibration::cold::PhyColdTransitionError> for PhyRegisterTransitionError {
    fn from(error: crate::calibration::cold::PhyColdTransitionError) -> Self {
        match error {
            crate::calibration::cold::PhyColdTransitionError::WrongCompletion => {
                Self::WrongCompletion
            }
            crate::calibration::cold::PhyColdTransitionError::AlreadyComplete => {
                Self::AlreadyComplete
            }
        }
    }
}

impl From<crate::calibration::baseband::PhyBbInitTransitionError> for PhyRegisterTransitionError {
    fn from(error: crate::calibration::baseband::PhyBbInitTransitionError) -> Self {
        match error {
            crate::calibration::baseband::PhyBbInitTransitionError::WrongCompletion => {
                Self::WrongCompletion
            }
            crate::calibration::baseband::PhyBbInitTransitionError::AlreadyComplete => {
                Self::AlreadyComplete
            }
        }
    }
}

/// Non-cloneable identity token for one finite parent MMIO operation.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRegisterMmioBinding {
    action: PhyRegisterMmioAction,
}

impl PhyRegisterMmioBinding {
    pub const fn new(action: PhyRegisterMmioAction) -> Self {
        Self { action }
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> PhyRegisterMmioAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target<P, R: oer_esp32s31_hal::owner::PhyInitializationAccess>(
        self,
        _platform: &mut P,
        registers: &mut R,
    ) -> PhyRegisterMmioCompletion {
        match self.action {
            PhyRegisterMmioAction::PrepareColdStart => {
                oer_esp32s31_hal::phy::frequency::prepare_common_phy_control(registers)
            }
            PhyRegisterMmioAction::ConfigureForceTxRx { enabled, phase } => {
                oer_esp32s31_hal::phy::pbus::configure_force_txrx(registers, enabled, phase)
            }
            PhyRegisterMmioAction::ResetFrequencyModule => {
                oer_esp32s31_hal::phy::frequency::reset_module(registers)
            }
            PhyRegisterMmioAction::SetHardwareFrequencyControl { enabled } => {
                oer_esp32s31_hal::phy::frequency::set_hardware_control(registers, enabled)
            }
            PhyRegisterMmioAction::PulseI2cMasterReset { index } => {
                let host = if index == 0 {
                    oer_esp32s31_hal::phy::i2c::PhyI2cHost::Host0
                } else {
                    oer_esp32s31_hal::phy::i2c::PhyI2cHost::Host1
                };
                oer_esp32s31_hal::phy::i2c::pulse_master_reset(registers, host)
            }
            PhyRegisterMmioAction::ConfigureXtal40Mhz => {
                oer_esp32s31_hal::phy::prelude::configure_fixed_xtal_40mhz(registers)
            }
            PhyRegisterMmioAction::SetCalibrationClock { enabled } => {
                crate::hardware::set_phy_register_calibration_clock(registers, enabled)
            }
            PhyRegisterMmioAction::SetBbpllCalibration { enabled } => {
                oer_esp32s31_hal::phy::i2c::configure_bbpll_calibration(registers, enabled)
            }
        }
        PhyRegisterMmioCompletion {
            action: self.action,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRegisterResetSampleBinding {
    index: u8,
    sample: u16,
}

impl PhyRegisterResetSampleBinding {
    pub fn new(action: PhyRegisterAction) -> Option<Self> {
        match action {
            PhyRegisterAction::SampleI2cMasterReset { index, sample } if index <= 1 => {
                Some(Self { index, sample })
            }
            _ => None,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target<P: oer_esp32s31_hal::owner::SharedPhyAccess>(
        self,
        registers: &mut P,
    ) -> PhyRegisterCompletion {
        let host = if self.index == 0 {
            oer_esp32s31_hal::phy::i2c::PhyI2cHost::Host0
        } else {
            oer_esp32s31_hal::phy::i2c::PhyI2cHost::Host1
        };
        PhyRegisterCompletion::I2cMasterResetSampled {
            index: self.index,
            sample: self.sample,
            busy: oer_esp32s31_hal::phy::i2c::sample_master_reset_busy(registers, host),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterBindingError {
    UnsupportedAction,
    IncompleteTransaction,
    UnexpectedOutcome,
}

/// Consumed identity for one parent delay completed by the Rust executor.
///
/// This type contains no clock source and cannot expire itself. The executor
/// owns the deadline and consumes this token only after its async timer edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRegisterTimerBinding {
    phase: PhyRegisterDelayPhase,
    micros: u32,
}

impl PhyRegisterTimerBinding {
    pub fn new(action: PhyRegisterAction) -> Result<Self, PhyRegisterBindingError> {
        match action {
            PhyRegisterAction::DelayMicros { phase, micros } => Ok(Self { phase, micros }),
            _ => Err(PhyRegisterBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyRegisterCompletion {
        PhyRegisterCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

/// Non-cloneable owner of the final one-byte PHY-I2C read.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRegisterFinalI2cBinding {
    address: crate::analog::i2c::PhyI2cAddress,
    transaction: crate::calibration::cold::PhyColdI2cTransaction,
}

impl PhyRegisterFinalI2cBinding {
    pub fn new(action: PhyRegisterAction) -> Result<Self, PhyRegisterBindingError> {
        let PhyRegisterAction::ReadFinalI2c { address } = action else {
            return Err(PhyRegisterBindingError::UnsupportedAction);
        };
        Ok(Self {
            address,
            transaction: crate::calibration::cold::PhyColdI2cTransaction::new(
                crate::calibration::cold::PhyColdI2cRequest::read_byte(address),
            ),
        })
    }

    pub const fn action(&self) -> crate::calibration::cold::PhyColdI2cAction {
        self.transaction.action()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn read_started(&mut self) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.read_started()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn observe_read_result(
        &mut self,
        result: Result<u8, crate::analog::i2c::PhyI2cError>,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_read_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target<P: oer_esp32s31_hal::owner::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.start_target(platform)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge<P: oer_esp32s31_hal::owner::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_target_edge(platform)
    }

    pub fn into_completion(self) -> Result<PhyRegisterCompletion, PhyRegisterBindingError> {
        match self.transaction.action() {
            crate::calibration::cold::PhyColdI2cAction::Complete(
                crate::calibration::cold::PhyColdI2cOutcome::Read { address, value },
            ) if address == self.address => {
                Ok(PhyRegisterCompletion::FinalI2cRead { address, value })
            }
            crate::calibration::cold::PhyColdI2cAction::Complete(_) => {
                Err(PhyRegisterBindingError::UnexpectedOutcome)
            }
            _ => Err(PhyRegisterBindingError::IncompleteTransaction),
        }
    }

    pub const fn into_deadline_completion(self) -> PhyRegisterCompletion {
        PhyRegisterCompletion::FinalI2cDeadlineExceeded {
            address: self.address,
        }
    }
}

/// Exhaustive source-owned lowering of `register_chipv7_phy`.
///
/// A successful construction proves that the next parent operation can be
/// driven without calling the vendor parent, consulting its callback table,
/// or performing a synchronous wait. Terminal state is intentionally not an
/// external binding.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyRegisterExternalBinding {
    Mmio(PhyRegisterMmioBinding),
    Timer(PhyRegisterTimerBinding),
    ResetSample(PhyRegisterResetSampleBinding),
    Rf(crate::calibration::cold::PhyColdExternalBinding),
    Baseband(crate::calibration::baseband::PhyBbExternalBinding),
    Temperature(crate::analog::temperature::PhyTemperatureExternalBinding),
    FinalI2c(PhyRegisterFinalI2cBinding),
}

impl PhyRegisterExternalBinding {
    pub fn lower(action: PhyRegisterAction) -> Result<Self, PhyRegisterBindingError> {
        match action {
            PhyRegisterAction::Mmio(action) => Ok(Self::Mmio(PhyRegisterMmioBinding::new(action))),
            PhyRegisterAction::DelayMicros { .. } => {
                PhyRegisterTimerBinding::new(action).map(Self::Timer)
            }
            PhyRegisterAction::SampleI2cMasterReset { .. } => {
                PhyRegisterResetSampleBinding::new(action)
                    .map(Self::ResetSample)
                    .ok_or(PhyRegisterBindingError::UnsupportedAction)
            }
            PhyRegisterAction::Rf(action) => {
                crate::calibration::cold::PhyColdExternalBinding::lower(action)
                    .map(Self::Rf)
                    .map_err(|_| PhyRegisterBindingError::UnsupportedAction)
            }
            PhyRegisterAction::Baseband(action) => {
                crate::calibration::baseband::PhyBbExternalBinding::lower(action)
                    .map(Self::Baseband)
                    .map_err(|_| PhyRegisterBindingError::UnsupportedAction)
            }
            PhyRegisterAction::Temperature(action) => {
                crate::analog::temperature::PhyTemperatureExternalBinding::lower(action)
                    .map(Self::Temperature)
                    .map_err(|_| PhyRegisterBindingError::UnsupportedAction)
            }
            PhyRegisterAction::ReadFinalI2c { .. } => {
                PhyRegisterFinalI2cBinding::new(action).map(Self::FinalI2c)
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(feature = "registration-diagnostics")]
mod diagnostics;
#[cfg(feature = "registration-diagnostics")]
pub use diagnostics::{
    FrequencyCalibrationDiagnostics, RfCalibrationDiagnostics, RfpllCalibrationPoint,
};
