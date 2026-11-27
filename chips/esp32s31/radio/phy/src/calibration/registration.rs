//! Rust-owned composition of ESP32-S31 `register_chipv7_phy`.
//!
//! The pinned archive parent is 486 bytes. Persistence remains outside the
//! radio driver: callers may provide and retrieve a typed calibration cache,
//! while this transition owns validation and calibration. A valid snapshot
//! seeds partial calibration; hardware-resident state is republished by the
//! ordinary cold RF/baseband graph.
//! All retained radio work is represented by owned state plus
//! one externally completed MMIO, PHY-I2C, timer or observation edge.

use oer_chips_esp32s31_hal::radio::phy::i2c::Host;
use oer_chips_esp32s31_hal::types::PhyBbpllCalibration;
use oer_chips_esp32s31_hal::types::PhyForceTxRxPhase;
use oer_chips_esp32s31_hal::types::PhyFrequencyControl;

pub const PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT: u16 = 1_000;

const PHY_REGISTER_FINAL_I2C_ADDRESS: crate::analog::i2c::Address =
    crate::analog::i2c::analog_i2c::sdm::update_enable::UPDATE_ENABLE.address();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterDelayPhase {
    ForceTxRx { enabled: bool, completed_phase: u8 },
    HardwareFrequencyDisabled,
    I2cMasterReset { host: Host, sample: u16 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterMmioAction {
    PrepareColdStart,
    ConfigureForceTxRx { phase: PhyForceTxRxPhase },
    ResetFrequencyModule,
    SetHardwareFrequencyControl { control: PhyFrequencyControl },
    PulseI2cMasterReset { host: Host },
    ConfigureXtal40Mhz,
    SetCalibrationClock { enabled: bool },
    SetBbpllCalibration { calibration: PhyBbpllCalibration },
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
        host: Host,
        sample: u16,
    },
    Rf(crate::analog::i2c::PhyRfInitPrefixAction),
    Baseband(crate::calibration::baseband::PhyBbInitAction),
    Temperature(crate::analog::temperature::PhyTemperatureAction),
    ReadFinalI2c {
        address: crate::analog::i2c::Address,
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
        host: Host,
        sample: u16,
        busy: bool,
    },
    Rf(crate::analog::i2c::PhyRfInitPrefixCompletion),
    Baseband(crate::calibration::baseband::PhyBbInitCompletion),
    Temperature(crate::analog::temperature::PhyTemperatureCompletion),
    FinalI2cRead {
        address: crate::analog::i2c::Address,
        value: u8,
    },
    FinalI2cDeadlineExceeded {
        address: crate::analog::i2c::Address,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterFailure {
    I2cMasterResetTimedOut { host: Host, samples: u16 },
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
    #[cfg(oer_trace)]
    pub rf_calibration: Option<RfCalibrationDiagnostics>,
    /// The longest not-ready runs of the bounded readiness waits.
    #[cfg(oer_trace)]
    pub ready_waits: ReadyWaitDiagnostics,
    pub full_calibration_performed: bool,
    pub calibration_path: PhyCalibrationPath,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRegisterLocalStep {
    StateAdvanced,
    /// The retained RX program is borrowed, never lowered through leaf envelopes.
    RxGain,
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
    I2cInitialSample { host: Host },
    I2cResetPulse { host: Host },
    I2cResetDelay { host: Host, sample: u16 },
    I2cResetSample { host: Host, sample: u16 },
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
    ApplyProfile,
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

/// Complete full-calibration state machine replacing the stateful vendor
/// parent, over the PHY state each step is lent.
pub struct PhyRegisterTransition {
    config: Option<crate::state::PhyConfig>,
    channel_or_frequency: u16,
    calibration_identity: Option<PhyCalibrationIdentity>,
    calibration_cache: Option<crate::state::PhyCalibrationCache>,
    calibration_candidate: bool,
    calibration_cache_ready: bool,
    calibration_path: PhyCalibrationPath,
    temperature_control: Option<crate::calibration::cold::PhyRegisterTemperatureControl>,
    #[cfg(oer_trace)]
    rf_calibration: Option<RfCalibrationDiagnostics>,
    #[cfg(oer_trace)]
    ready_waits: ReadyWaitDiagnostics,
    phase: Option<Phase>,
}

impl PhyRegisterTransition {
    #[cfg(test)]
    pub const fn new(config: crate::state::PhyConfig) -> Self {
        Self::new_on_channel(config, 11)
    }

    #[cfg(test)]
    pub const fn new_on_channel(
        config: crate::state::PhyConfig,
        channel_or_frequency: u16,
    ) -> Self {
        Self {
            config: Some(config),
            channel_or_frequency,
            calibration_identity: None,
            calibration_cache: None,
            calibration_candidate: false,
            calibration_cache_ready: false,
            calibration_path: PhyCalibrationPath::FullUncached,
            temperature_control: None,
            #[cfg(oer_trace)]
            rf_calibration: None,
            #[cfg(oer_trace)]
            ready_waits: ReadyWaitDiagnostics {
                tx_dc_max_not_ready_samples: 0,
                pwdet_sar_max_not_ready_samples: 0,
            },
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
            #[cfg(oer_trace)]
            rf_calibration: None,
            #[cfg(oer_trace)]
            ready_waits: ReadyWaitDiagnostics {
                tx_dc_max_not_ready_samples: 0,
                pwdet_sar_max_not_ready_samples: 0,
            },
            phase: Some(Phase::Prelude(PreludeStep::Prepare)),
        }
    }

    #[cfg(test)]
    pub const fn with_production_config() -> Self {
        Self::new(crate::state::PhyConfig::production())
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

    /// Return the freshly completed full- or partial-calibration cache.
    ///
    /// Failed and in-progress transitions never expose a cache as persistable.
    #[cfg(test)]
    pub fn calibration_cache(&self) -> Option<&crate::state::PhyCalibrationCache> {
        (self.calibration_cache_ready && matches!(self.phase.as_ref(), Some(Phase::Complete(_))))
            .then_some(())
            .and(self.calibration_cache.as_ref())
    }

    /// The cache of a completed transition, when the registration made
    /// one. Every non-success path returns the transition unchanged.
    #[allow(
        clippy::result_large_err,
        reason = "failure must return the unique allocation-free registration owner"
    )]
    pub fn into_completed_cache(
        mut self,
    ) -> Result<Option<crate::state::PhyCalibrationCache>, Self> {
        if !matches!(self.phase.as_ref(), Some(Phase::Complete(_))) {
            return Err(self);
        }
        Ok(if self.calibration_cache_ready {
            self.calibration_cache.take()
        } else {
            None
        })
    }

    /// Whether failure cleanup reached its terminal phase with the state owner
    /// intact. This is a model fact, not independent proof of target MMIO; target
    /// runners may expose it only for their own executed transition.
    #[cfg(test)]
    pub fn failure_cleanup_completed(&self) -> bool {
        matches!(self.phase.as_ref(), Some(Phase::Failed(_)))
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
    #[cfg(test)]
    pub fn into_failed_parts(mut self) -> Result<Option<crate::state::PhyCalibrationCache>, Self> {
        if !self.failure_cleanup_completed() {
            return Err(self);
        }
        Ok(self.calibration_cache.take())
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
                PhyRegisterMmioAction::SetBbpllCalibration {
                    calibration: PhyBbpllCalibration::Disabled,
                },
            )),
            TailStep::FinalI2cRead => Some(PhyRegisterAction::ReadFinalI2c {
                address: PHY_REGISTER_FINAL_I2C_ADDRESS,
            }),
            TailStep::EnableHardwareFrequency => Some(PhyRegisterAction::Mmio(
                PhyRegisterMmioAction::SetHardwareFrequencyControl {
                    control: PhyFrequencyControl::Hardware,
                },
            )),
            TailStep::ReleaseFirst => Some(PhyRegisterAction::Mmio(
                PhyRegisterMmioAction::ConfigureForceTxRx {
                    phase: PhyForceTxRxPhase::DisableInitialPhase,
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
                    phase: PhyForceTxRxPhase::DisableFinalPhase,
                },
            )),
            TailStep::ReleaseSecondDelay => Some(PhyRegisterAction::DelayMicros {
                phase: PhyRegisterDelayPhase::ForceTxRx {
                    enabled: false,
                    completed_phase: 1,
                },
                micros: 1,
            }),
            TailStep::Temperature | TailStep::BackupCalibration => None,
        }
    }

    pub(crate) fn rx_program_mut(&mut self) -> Option<&mut crate::rx::gain::RxProgram> {
        match self.phase.as_mut() {
            Some(Phase::Baseband(parent)) => parent.rx_program_mut(),
            _ => None,
        }
    }

    // Keep phase temporaries in this synchronous model step, rather than in
    // every registration future's poll frame.
    #[inline(never)]
    pub fn step_local(
        &mut self,
        state: &mut crate::state::PhyState,
    ) -> Result<PhyRegisterLocalStep, PhyRegisterTransitionError> {
        let phase = self
            .phase
            .take()
            .ok_or(PhyRegisterTransitionError::AlreadyComplete)?;
        match phase {
            Phase::ApplyProfile => {
                let Some(config) = self.config.take() else {
                    self.phase = Some(Phase::ApplyProfile);
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
                            phase: PhyForceTxRxPhase::EnableInitialPhase,
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
                            phase: PhyForceTxRxPhase::EnableFinalPhase,
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
                        PhyRegisterMmioAction::SetHardwareFrequencyControl {
                            control: PhyFrequencyControl::Software,
                        },
                    ),
                    PreludeStep::DisableHardwareFrequencyDelay => PhyRegisterAction::DelayMicros {
                        phase: PhyRegisterDelayPhase::HardwareFrequencyDisabled,
                        micros: 2,
                    },
                    PreludeStep::I2cInitialSample { host }
                    | PreludeStep::I2cResetSample { host, sample: 0 } => {
                        PhyRegisterAction::SampleI2cMasterReset { host, sample: 0 }
                    }
                    PreludeStep::I2cResetSample { host, sample } => {
                        PhyRegisterAction::SampleI2cMasterReset { host, sample }
                    }
                    PreludeStep::I2cResetPulse { host } => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::PulseI2cMasterReset { host })
                    }
                    PreludeStep::I2cResetDelay { host, sample } => PhyRegisterAction::DelayMicros {
                        phase: PhyRegisterDelayPhase::I2cMasterReset { host, sample },
                        micros: 1,
                    },
                    PreludeStep::ConfigureXtal => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::ConfigureXtal40Mhz)
                    }
                    PreludeStep::CalibrationClockOn => {
                        PhyRegisterAction::Mmio(PhyRegisterMmioAction::SetCalibrationClock {
                            enabled: true,
                        })
                    }
                };
                self.phase = Some(Phase::Prelude(step));
                Ok(PhyRegisterLocalStep::External(action))
            }
            Phase::Rf(mut transition) => {
                let local = match transition.step_local(state) {
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
                        #[cfg(oer_trace)]
                        {
                            self.rf_calibration = RfCalibrationDiagnostics::from_outcome(outcome);
                        }
                        if successful {
                            self.phase = Some(Phase::Baseband(
                                crate::calibration::baseband::PhyBbInitTransition::new_on_channel(
                                    self.channel_or_frequency,
                                ),
                            ));
                        } else {
                            self.begin_cleanup(PhyRegisterFailure::Rf(outcome));
                        }
                        Ok(PhyRegisterLocalStep::StateAdvanced)
                    }
                }
            }
            Phase::Baseband(mut transition) => {
                let local = match transition.step_local(state) {
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
                    crate::calibration::baseband::PhyBbInitLocalStep::RxGain => {
                        self.phase = Some(Phase::Baseband(transition));
                        Ok(PhyRegisterLocalStep::RxGain)
                    }
                    crate::calibration::baseband::PhyBbInitLocalStep::External(action) => {
                        self.phase = Some(Phase::Baseband(transition));
                        Ok(PhyRegisterLocalStep::External(PhyRegisterAction::Baseband(
                            action,
                        )))
                    }
                    crate::calibration::baseband::PhyBbInitLocalStep::Complete(_) => {
                        self.phase = Some(Phase::Tail(TailStep::CalibrationClockOff));
                        Ok(PhyRegisterLocalStep::StateAdvanced)
                    }
                    crate::calibration::baseband::PhyBbInitLocalStep::Failed(failure) => {
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
            Phase::Tail(step) => {
                let action =
                    Self::tail_action(step).ok_or(PhyRegisterTransitionError::WrongCompletion)?;
                self.phase = Some(Phase::Tail(step));
                Ok(PhyRegisterLocalStep::External(action))
            }
            Phase::Cleanup {
                step: TailStep::Temperature | TailStep::BackupCalibration | TailStep::FinalI2cRead,
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

    #[inline(never)]
    pub fn advance_external(
        &mut self,
        state: &mut crate::state::PhyState,
        completion: PhyRegisterCompletion,
    ) -> Result<(), PhyRegisterTransitionError> {
        let phase = self
            .phase
            .take()
            .ok_or(PhyRegisterTransitionError::AlreadyComplete)?;
        #[cfg(oer_trace)]
        self.ready_waits.observe(&completion);
        let next = match (phase, completion) {
            (Phase::Prelude(PreludeStep::Prepare), PhyRegisterCompletion::Mmio(completed))
                if completed.action == PhyRegisterMmioAction::PrepareColdStart =>
            {
                Phase::Prelude(PreludeStep::ForceFirst)
            }
            (Phase::Prelude(PreludeStep::ForceFirst), PhyRegisterCompletion::Mmio(completed))
                if completed.action
                    == (PhyRegisterMmioAction::ConfigureForceTxRx {
                        phase: PhyForceTxRxPhase::EnableInitialPhase,
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
                        phase: PhyForceTxRxPhase::EnableFinalPhase,
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
                == (PhyRegisterMmioAction::SetHardwareFrequencyControl {
                    control: PhyFrequencyControl::Software,
                }) =>
            {
                Phase::Prelude(PreludeStep::DisableHardwareFrequencyDelay)
            }
            (
                Phase::Prelude(PreludeStep::DisableHardwareFrequencyDelay),
                PhyRegisterCompletion::DelayElapsed {
                    phase: PhyRegisterDelayPhase::HardwareFrequencyDisabled,
                    micros: 2,
                },
            ) => Phase::Prelude(PreludeStep::I2cInitialSample { host: Host::Host0 }),
            (
                Phase::Prelude(PreludeStep::I2cInitialSample { host }),
                PhyRegisterCompletion::I2cMasterResetSampled {
                    host: completed_host,
                    sample: 0,
                    busy,
                },
            ) if completed_host == host => {
                if busy {
                    Phase::Prelude(PreludeStep::I2cResetPulse { host })
                } else {
                    match host {
                        Host::Host0 => {
                            Phase::Prelude(PreludeStep::I2cInitialSample { host: Host::Host1 })
                        }
                        Host::Host1 => Phase::ApplyProfile,
                    }
                }
            }
            (
                Phase::Prelude(PreludeStep::I2cResetPulse { host }),
                PhyRegisterCompletion::Mmio(completed),
            ) if completed.action == PhyRegisterMmioAction::PulseI2cMasterReset { host } => {
                Phase::Prelude(PreludeStep::I2cResetDelay { host, sample: 0 })
            }
            (
                Phase::Prelude(PreludeStep::I2cResetDelay { host, sample }),
                PhyRegisterCompletion::DelayElapsed {
                    phase:
                        PhyRegisterDelayPhase::I2cMasterReset {
                            host: completed_host,
                            sample: completed_sample,
                        },
                    micros: 1,
                },
            ) if completed_host == host && completed_sample == sample => {
                Phase::Prelude(PreludeStep::I2cResetSample { host, sample })
            }
            (
                Phase::Prelude(PreludeStep::I2cResetSample { host, sample }),
                PhyRegisterCompletion::I2cMasterResetSampled {
                    host: completed_host,
                    sample: completed_sample,
                    busy,
                },
            ) if completed_host == host && completed_sample == sample => {
                if !busy {
                    match host {
                        Host::Host0 => {
                            Phase::Prelude(PreludeStep::I2cInitialSample { host: Host::Host1 })
                        }
                        Host::Host1 => Phase::ApplyProfile,
                    }
                } else if sample + 1 == PHY_REGISTER_I2C_RESET_SAMPLE_LIMIT {
                    self.begin_cleanup(PhyRegisterFailure::I2cMasterResetTimedOut {
                        host,
                        samples: sample + 1,
                    });
                    return Ok(());
                } else {
                    Phase::Prelude(PreludeStep::I2cResetDelay {
                        host,
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
                Phase::Rf(crate::calibration::cold::PhyRfColdInit::new())
            }
            (Phase::Rf(mut transition), PhyRegisterCompletion::Rf(completed)) => {
                if let Err(error) = transition.advance_external(state, completed) {
                    self.phase = Some(Phase::Rf(transition));
                    return Err(error.into());
                }
                Phase::Rf(transition)
            }
            (Phase::Baseband(mut transition), PhyRegisterCompletion::Baseband(completed)) => {
                if let Err(error) = transition.advance_external(state, completed) {
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
                    == (PhyRegisterMmioAction::SetBbpllCalibration {
                        calibration: PhyBbpllCalibration::Disabled,
                    }) =>
            {
                Phase::Tail(TailStep::FinalI2cRead)
            }
            (
                Phase::Tail(TailStep::FinalI2cRead),
                PhyRegisterCompletion::FinalI2cRead { address, .. },
            ) if address == PHY_REGISTER_FINAL_I2C_ADDRESS => {
                Phase::Tail(TailStep::EnableHardwareFrequency)
            }
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
                == (PhyRegisterMmioAction::SetHardwareFrequencyControl {
                    control: PhyFrequencyControl::Hardware,
                }) =>
            {
                Phase::Tail(TailStep::ReleaseFirst)
            }
            (Phase::Tail(TailStep::ReleaseFirst), PhyRegisterCompletion::Mmio(completed))
                if completed.action
                    == (PhyRegisterMmioAction::ConfigureForceTxRx {
                        phase: PhyForceTxRxPhase::DisableInitialPhase,
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
                        phase: PhyForceTxRxPhase::DisableFinalPhase,
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
                    self.calibration_cache = Some(state.calibration_cache(identity));
                    self.calibration_cache_ready = true;
                }
                Phase::Complete(PhyRegisterOutcome {
                    #[cfg(oer_trace)]
                    rf_calibration: self.rf_calibration,
                    #[cfg(oer_trace)]
                    ready_waits: self.ready_waits,
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
                == (PhyRegisterMmioAction::SetBbpllCalibration {
                    calibration: PhyBbpllCalibration::Disabled,
                }) =>
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
                == (PhyRegisterMmioAction::SetHardwareFrequencyControl {
                    control: PhyFrequencyControl::Hardware,
                }) =>
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
                    phase: PhyForceTxRxPhase::DisableInitialPhase,
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
                    phase: PhyForceTxRxPhase::DisableFinalPhase,
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

    #[cfg(test)]
    pub const fn action(&self) -> PhyRegisterMmioAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target<P, R: oer_chips_esp32s31_hal::system::owner::PhyInitializationAccess>(
        self,
        _platform: &mut P,
        registers: &mut R,
    ) -> PhyRegisterMmioCompletion {
        match self.action {
            PhyRegisterMmioAction::PrepareColdStart => {
                oer_chips_esp32s31_hal::radio::phy::frequency::prepare_common_phy_control(registers)
            }
            PhyRegisterMmioAction::ConfigureForceTxRx { phase } => {
                oer_chips_esp32s31_hal::radio::phy::pbus::configure_force_txrx(registers, phase)
            }
            PhyRegisterMmioAction::ResetFrequencyModule => {
                oer_chips_esp32s31_hal::radio::phy::frequency::reset_module(registers)
            }
            PhyRegisterMmioAction::SetHardwareFrequencyControl { control } => {
                oer_chips_esp32s31_hal::radio::phy::frequency::set_hardware_control(
                    registers, control,
                )
            }
            PhyRegisterMmioAction::PulseI2cMasterReset { host } => {
                oer_chips_esp32s31_hal::radio::phy::i2c::pulse_master_reset(registers, host)
            }
            PhyRegisterMmioAction::ConfigureXtal40Mhz => {
                oer_chips_esp32s31_hal::radio::phy::prelude::configure_fixed_xtal_40mhz(registers)
            }
            PhyRegisterMmioAction::SetCalibrationClock { enabled } => {
                crate::hardware::set_phy_register_calibration_clock(registers, enabled)
            }
            PhyRegisterMmioAction::SetBbpllCalibration { calibration } => {
                oer_chips_esp32s31_hal::radio::phy::i2c::configure_bbpll_calibration(
                    registers,
                    calibration,
                )
            }
        }
        PhyRegisterMmioCompletion {
            action: self.action,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRegisterResetSampleBinding {
    host: Host,
    sample: u16,
}

impl PhyRegisterResetSampleBinding {
    pub const fn new(host: Host, sample: u16) -> Self {
        Self { host, sample }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target<P: oer_chips_esp32s31_hal::system::owner::SharedPhyAccess>(
        self,
        registers: &mut P,
    ) -> PhyRegisterCompletion {
        PhyRegisterCompletion::I2cMasterResetSampled {
            host: self.host,
            sample: self.sample,
            busy: oer_chips_esp32s31_hal::radio::phy::i2c::sample_master_reset_busy(
                registers, self.host,
            ),
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
    address: crate::analog::i2c::Address,
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

    #[cfg(test)]
    pub fn read_started(&mut self) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.read_started()
    }

    #[cfg(test)]
    pub fn observe_read_result(
        &mut self,
        result: Result<u8, crate::serial::PhyI2cError>,
    ) -> Result<
        crate::calibration::cold::PhyColdI2cObservation,
        crate::calibration::cold::PhyColdI2cError,
    > {
        self.transaction.observe_read_result(result)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target<P: oer_chips_esp32s31_hal::system::owner::SharedPhyAccess>(
        &mut self,
        platform: &mut P,
    ) -> Result<(), crate::calibration::cold::PhyColdI2cError> {
        self.transaction.start_target(platform)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge<P: oer_chips_esp32s31_hal::system::owner::SharedPhyAccess>(
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
            PhyRegisterAction::SampleI2cMasterReset { host, sample } => Ok(Self::ResetSample(
                PhyRegisterResetSampleBinding::new(host, sample),
            )),
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

#[cfg(oer_trace)]
mod diagnostics;
#[cfg(oer_trace)]
pub use diagnostics::{
    FrequencyCalibrationDiagnostics, ReadyWaitDiagnostics, RfCalibrationDiagnostics,
    RfpllCalibrationPoint,
};
