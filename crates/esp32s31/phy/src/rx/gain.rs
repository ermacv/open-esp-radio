//! Rust-owned RX-gain table generation and hardware-memory publication.
//!
//! This module covers the pure ROM `phy_gen_rx_gain_table` child and the
//! complete pinned `phy_wr_rx_gain_mem_new` loop used by
//! `phy_set_rx_gain_table(0x985, 0)`. RX-DC and RX-IQ calibration remain
//! separate typed predecessors; this publisher accepts only copied outcomes.

use crate::{
    analog::pbus::PhyPbusForceTest,
    calibration::baseband::{
        PhyGainMemoryEntry, PhyGeneratedRxGainTable, PhyRxGainBank, PhyRxGainMemoryParameters,
        generate_phy_rx_gain_table, phy_generated_rx_gain_memory_entry,
    },
    rx::gain_calibration::{
        PhyRxGainDcAction, PhyRxGainDcCompletion, PhyRxGainDcFailure, PhyRxGainDcOutcome,
        PhyRxGainDcParameters, PhyRxGainDcTransition,
    },
};

const PBUS_RX_ON_COUNT: u8 = 7;

/// Fixed gain encodings depend only on the hardware bank. They are generated
/// once at compile time; measured DC corrections remain in each PHY owner.
const fn generated_table(bank: PhyRxGainBank) -> &'static PhyGeneratedRxGainTable {
    static WIFI: PhyGeneratedRxGainTable = generate_phy_rx_gain_table(PhyRxGainBank::Wifi);
    static SHARED: PhyGeneratedRxGainTable = generate_phy_rx_gain_table(PhyRxGainBank::Shared);
    match bank {
        PhyRxGainBank::Wifi => &WIFI,
        PhyRxGainBank::Shared => &SHARED,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainClock {
    Rx,
    Tx,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainDelayPhase {
    PbusWorkMode { bank: PhyRxGainBank },
    PbusWorkModePulse { bank: PhyRxGainBank },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainPublishOutcome {
    pub wifi_entries: u8,
    pub shared_entries: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainPublishFailure {
    PbusTimedOut {
        bank: PhyRxGainBank,
        transaction: PhyPbusForceTest,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainPublishAction {
    ConfigurePbusDebugMode {
        bank: PhyRxGainBank,
    },
    ForcePbus {
        bank: PhyRxGainBank,
        transaction: PhyPbusForceTest,
    },
    ConfigureClock {
        bank: PhyRxGainBank,
        clock: PhyRxGainClock,
        enabled: bool,
    },
    ProgramEntry {
        bank: PhyRxGainBank,
        entry: PhyGainMemoryEntry,
    },
    ConfigurePbusWorkMode {
        bank: PhyRxGainBank,
    },
    DelayMicros {
        phase: PhyRxGainDelayPhase,
        micros: u32,
    },
    ConfigurePbusWorkModePulse {
        bank: PhyRxGainBank,
    },
    ClearPbusWorkModePulse {
        bank: PhyRxGainBank,
    },
    Complete(PhyRxGainPublishOutcome),
    Failed(PhyRxGainPublishFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainPublishCompletion {
    PbusDebugModeConfigured {
        bank: PhyRxGainBank,
    },
    PbusCompleted {
        bank: PhyRxGainBank,
        transaction: PhyPbusForceTest,
    },
    PbusTimedOut {
        bank: PhyRxGainBank,
        transaction: PhyPbusForceTest,
    },
    ClockConfigured {
        bank: PhyRxGainBank,
        clock: PhyRxGainClock,
        enabled: bool,
    },
    EntryProgrammed {
        bank: PhyRxGainBank,
        entry: PhyGainMemoryEntry,
    },
    PbusWorkModeConfigured {
        bank: PhyRxGainBank,
        settle_required: bool,
    },
    DelayElapsed {
        phase: PhyRxGainDelayPhase,
        micros: u32,
    },
    PbusWorkModePulseConfigured {
        bank: PhyRxGainBank,
    },
    PbusWorkModePulseCleared {
        bank: PhyRxGainBank,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainPublishTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainPublishBindingError {
    NotDirectMmio,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    Debug(PhyRxGainBank),
    RxOn {
        bank: PhyRxGainBank,
        index: u8,
        restore: bool,
    },
    Clock {
        bank: PhyRxGainBank,
        clock: PhyRxGainClock,
        enabled: bool,
    },
    Entries {
        bank: PhyRxGainBank,
        index: u8,
    },
    WorkMode {
        bank: PhyRxGainBank,
        failure: Option<PhyRxGainPublishFailure>,
    },
    WorkModeDelay {
        bank: PhyRxGainBank,
        failure: Option<PhyRxGainPublishFailure>,
    },
    WorkModePulse {
        bank: PhyRxGainBank,
        failure: Option<PhyRxGainPublishFailure>,
    },
    WorkModePulseDelay {
        bank: PhyRxGainBank,
        failure: Option<PhyRxGainPublishFailure>,
    },
    WorkModePulseClear {
        bank: PhyRxGainBank,
        failure: Option<PhyRxGainPublishFailure>,
    },
    Complete,
    Failed(PhyRxGainPublishFailure),
}

const fn pbus_rx_on(index: u8, pbus_rx_path_value: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(4, 2, 1),
        2 => PhyPbusForceTest::new(5, 1, 0),
        3 => PhyPbusForceTest::new(0, 1, 0x40),
        4 => PhyPbusForceTest::new(0, 2, pbus_rx_path_value as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x189),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

const fn next_bank(bank: PhyRxGainBank) -> Step {
    match bank {
        PhyRxGainBank::Wifi => Step::Debug(PhyRxGainBank::Shared),
        PhyRxGainBank::Shared => Step::Complete,
    }
}

const fn after_work_mode(bank: PhyRxGainBank, failure: Option<PhyRxGainPublishFailure>) -> Step {
    match failure {
        Some(failure) => Step::Failed(failure),
        None => next_bank(bank),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainPublishTransition {
    parameters: PhyRxGainMemoryParameters,
    step: Step,
}

impl PhyRxGainPublishTransition {
    pub fn new(parameters: PhyRxGainMemoryParameters) -> Self {
        Self {
            parameters,
            step: Step::Debug(PhyRxGainBank::Wifi),
        }
    }

    pub fn action(&self) -> PhyRxGainPublishAction {
        match self.step {
            Step::Debug(bank) => PhyRxGainPublishAction::ConfigurePbusDebugMode { bank },
            Step::RxOn {
                bank,
                index,
                restore: _,
            } => PhyRxGainPublishAction::ForcePbus {
                bank,
                transaction: pbus_rx_on(index, self.parameters.parameter_002),
            },
            Step::Clock {
                bank,
                clock,
                enabled,
            } => PhyRxGainPublishAction::ConfigureClock {
                bank,
                clock,
                enabled,
            },
            Step::Entries { bank, index } => PhyRxGainPublishAction::ProgramEntry {
                bank,
                entry: phy_generated_rx_gain_memory_entry(
                    &self.parameters,
                    bank,
                    generated_table(bank),
                    index,
                ),
            },
            Step::WorkMode { bank, .. } => PhyRxGainPublishAction::ConfigurePbusWorkMode { bank },
            Step::WorkModeDelay { bank, .. } => PhyRxGainPublishAction::DelayMicros {
                phase: PhyRxGainDelayPhase::PbusWorkMode { bank },
                micros: 1,
            },
            Step::WorkModePulse { bank, .. } => {
                PhyRxGainPublishAction::ConfigurePbusWorkModePulse { bank }
            }
            Step::WorkModePulseDelay { bank, .. } => PhyRxGainPublishAction::DelayMicros {
                phase: PhyRxGainDelayPhase::PbusWorkModePulse { bank },
                micros: 2,
            },
            Step::WorkModePulseClear { bank, .. } => {
                PhyRxGainPublishAction::ClearPbusWorkModePulse { bank }
            }
            Step::Complete => PhyRxGainPublishAction::Complete(PhyRxGainPublishOutcome {
                wifi_entries: generated_table(PhyRxGainBank::Wifi).last_index + 1,
                shared_entries: generated_table(PhyRxGainBank::Shared).last_index + 1,
            }),
            Step::Failed(failure) => PhyRxGainPublishAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxGainPublishCompletion,
    ) -> Result<(), PhyRxGainPublishTransitionError> {
        self.step = match (self.step, completion) {
            (
                Step::Debug(expected),
                PhyRxGainPublishCompletion::PbusDebugModeConfigured { bank },
            ) if bank == expected => Step::RxOn {
                bank,
                index: 0,
                restore: false,
            },
            (
                Step::RxOn {
                    bank,
                    index,
                    restore,
                },
                PhyRxGainPublishCompletion::PbusCompleted {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank
                && transaction == pbus_rx_on(index, self.parameters.parameter_002) =>
            {
                if index + 1 == PBUS_RX_ON_COUNT && restore {
                    Step::WorkMode {
                        bank,
                        failure: None,
                    }
                } else if index + 1 == PBUS_RX_ON_COUNT {
                    Step::Clock {
                        bank,
                        clock: PhyRxGainClock::Rx,
                        enabled: true,
                    }
                } else {
                    Step::RxOn {
                        bank,
                        index: index + 1,
                        restore,
                    }
                }
            }
            (
                Step::RxOn {
                    bank,
                    index,
                    restore: _,
                },
                PhyRxGainPublishCompletion::PbusTimedOut {
                    bank: completed_bank,
                    transaction,
                },
            ) if bank == completed_bank
                && transaction == pbus_rx_on(index, self.parameters.parameter_002) =>
            {
                Step::WorkMode {
                    bank,
                    failure: Some(PhyRxGainPublishFailure::PbusTimedOut { bank, transaction }),
                }
            }
            (
                Step::Clock {
                    bank,
                    clock: PhyRxGainClock::Rx,
                    enabled: true,
                },
                PhyRxGainPublishCompletion::ClockConfigured {
                    bank: completed_bank,
                    clock: PhyRxGainClock::Rx,
                    enabled: true,
                },
            ) if bank == completed_bank => Step::Clock {
                bank,
                clock: PhyRxGainClock::Tx,
                enabled: true,
            },
            (
                Step::Clock {
                    bank,
                    clock: PhyRxGainClock::Tx,
                    enabled: true,
                },
                PhyRxGainPublishCompletion::ClockConfigured {
                    bank: completed_bank,
                    clock: PhyRxGainClock::Tx,
                    enabled: true,
                },
            ) if bank == completed_bank => Step::Entries { bank, index: 0 },
            (
                Step::Entries { bank, index },
                PhyRxGainPublishCompletion::EntryProgrammed {
                    bank: completed_bank,
                    entry,
                },
            ) if bank == completed_bank
                && entry
                    == phy_generated_rx_gain_memory_entry(
                        &self.parameters,
                        bank,
                        generated_table(bank),
                        index,
                    ) =>
            {
                if index == generated_table(bank).last_index {
                    Step::Clock {
                        bank,
                        clock: PhyRxGainClock::Rx,
                        enabled: false,
                    }
                } else {
                    Step::Entries {
                        bank,
                        index: index + 1,
                    }
                }
            }
            (
                Step::Clock {
                    bank,
                    clock: PhyRxGainClock::Rx,
                    enabled: false,
                },
                PhyRxGainPublishCompletion::ClockConfigured {
                    bank: completed_bank,
                    clock: PhyRxGainClock::Rx,
                    enabled: false,
                },
            ) if bank == completed_bank => Step::Clock {
                bank,
                clock: PhyRxGainClock::Tx,
                enabled: false,
            },
            (
                Step::Clock {
                    bank,
                    clock: PhyRxGainClock::Tx,
                    enabled: false,
                },
                PhyRxGainPublishCompletion::ClockConfigured {
                    bank: completed_bank,
                    clock: PhyRxGainClock::Tx,
                    enabled: false,
                },
            ) if bank == completed_bank => Step::RxOn {
                bank,
                index: 0,
                restore: true,
            },
            (
                Step::WorkMode { bank, failure },
                PhyRxGainPublishCompletion::PbusWorkModeConfigured {
                    bank: completed_bank,
                    settle_required,
                },
            ) if bank == completed_bank => {
                if settle_required {
                    Step::WorkModeDelay { bank, failure }
                } else {
                    after_work_mode(bank, failure)
                }
            }
            (
                Step::WorkModeDelay { bank, failure },
                PhyRxGainPublishCompletion::DelayElapsed {
                    phase:
                        PhyRxGainDelayPhase::PbusWorkMode {
                            bank: completed_bank,
                        },
                    micros: 1,
                },
            ) if bank == completed_bank => Step::WorkModePulse { bank, failure },
            (
                Step::WorkModePulse { bank, failure },
                PhyRxGainPublishCompletion::PbusWorkModePulseConfigured {
                    bank: completed_bank,
                },
            ) if bank == completed_bank => Step::WorkModePulseDelay { bank, failure },
            (
                Step::WorkModePulseDelay { bank, failure },
                PhyRxGainPublishCompletion::DelayElapsed {
                    phase:
                        PhyRxGainDelayPhase::PbusWorkModePulse {
                            bank: completed_bank,
                        },
                    micros: 2,
                },
            ) if bank == completed_bank => Step::WorkModePulseClear { bank, failure },
            (
                Step::WorkModePulseClear { bank, failure },
                PhyRxGainPublishCompletion::PbusWorkModePulseCleared {
                    bank: completed_bank,
                },
            ) if bank == completed_bank => after_work_mode(bank, failure),
            (Step::Complete | Step::Failed(_), _) => {
                return Err(PhyRxGainPublishTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxGainPublishTransitionError::WrongCompletion),
        };

        Ok(())
    }
}

/// Non-cloneable token for one finite non-PBus RX-gain operation.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxGainPublishMmioBinding {
    action: PhyRxGainPublishAction,
}

impl PhyRxGainPublishMmioBinding {
    pub fn new(action: PhyRxGainPublishAction) -> Result<Self, PhyRxGainPublishBindingError> {
        match action {
            PhyRxGainPublishAction::ConfigurePbusDebugMode { .. }
            | PhyRxGainPublishAction::ConfigureClock { .. }
            | PhyRxGainPublishAction::ProgramEntry { .. }
            | PhyRxGainPublishAction::ConfigurePbusWorkMode { .. }
            | PhyRxGainPublishAction::ConfigurePbusWorkModePulse { .. }
            | PhyRxGainPublishAction::ClearPbusWorkModePulse { .. } => Ok(Self { action }),
            _ => Err(PhyRxGainPublishBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> PhyRxGainPublishCompletion {
        match self.action {
            PhyRxGainPublishAction::ConfigurePbusDebugMode { bank } => {
                oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
                PhyRxGainPublishCompletion::PbusDebugModeConfigured { bank }
            }
            PhyRxGainPublishAction::ConfigureClock {
                bank,
                clock,
                enabled,
            } => {
                match clock {
                    PhyRxGainClock::Rx => {
                        oer_esp32s31_hal::phy::pbus::configure_rx_clock(registers, enabled)
                    }
                    PhyRxGainClock::Tx => {
                        oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, enabled)
                    }
                }
                PhyRxGainPublishCompletion::ClockConfigured {
                    bank,
                    clock,
                    enabled,
                }
            }
            PhyRxGainPublishAction::ProgramEntry { bank, entry } => {
                oer_esp32s31_hal::phy::memory::program_gain_memory_entry(registers, entry);
                PhyRxGainPublishCompletion::EntryProgrammed { bank, entry }
            }
            PhyRxGainPublishAction::ConfigurePbusWorkMode { bank } => {
                PhyRxGainPublishCompletion::PbusWorkModeConfigured {
                    bank,
                    settle_required: oer_esp32s31_hal::phy::pbus::configure_work_mode(registers),
                }
            }
            PhyRxGainPublishAction::ConfigurePbusWorkModePulse { bank } => {
                oer_esp32s31_hal::phy::agc::configure_pbus_work_mode_pulse(registers);
                PhyRxGainPublishCompletion::PbusWorkModePulseConfigured { bank }
            }
            PhyRxGainPublishAction::ClearPbusWorkModePulse { bank } => {
                oer_esp32s31_hal::phy::agc::clear_pbus_work_mode_pulse(registers);
                PhyRxGainPublishCompletion::PbusWorkModePulseCleared { bank }
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainExternalBindingError {
    UnsupportedAction,
    Pbus(crate::analog::pbus::PhyPbusHardwareBindingError),
}

/// Linear owner of one bank-qualified PBus publication edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxGainPublishPbusBinding {
    bank: PhyRxGainBank,
    transaction: PhyPbusForceTest,
    hardware: crate::analog::pbus::PhyPbusHardwareBinding,
}

impl PhyRxGainPublishPbusBinding {
    pub fn new(action: PhyRxGainPublishAction) -> Result<Self, PhyRxGainExternalBindingError> {
        let PhyRxGainPublishAction::ForcePbus { bank, transaction } = action else {
            return Err(PhyRxGainExternalBindingError::UnsupportedAction);
        };
        Ok(Self {
            bank,
            transaction,
            hardware: crate::analog::pbus::PhyPbusHardwareBinding::new(transaction),
        })
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> crate::analog::pbus::PhyPbusHardwareAction {
        self.hardware.action()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn started(&mut self) -> Result<(), crate::analog::pbus::PhyPbusHardwareBindingError> {
        self.hardware.started()
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub fn observe_completed(
        &mut self,
        completed: bool,
    ) -> Result<
        crate::analog::pbus::PhyPbusHardwareObservation,
        crate::analog::pbus::PhyPbusHardwareBindingError,
    > {
        self.hardware.observe_completed(completed)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<(), crate::analog::pbus::PhyPbusHardwareBindingError> {
        self.hardware.start_target(registers)
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<
        crate::analog::pbus::PhyPbusHardwareObservation,
        crate::analog::pbus::PhyPbusHardwareBindingError,
    > {
        self.hardware.observe_target_edge(registers)
    }

    pub fn into_completion(
        self,
    ) -> Result<PhyRxGainPublishCompletion, PhyRxGainExternalBindingError> {
        let bank = self.bank;
        self.hardware
            .into_transaction()
            .map(|transaction| PhyRxGainPublishCompletion::PbusCompleted { bank, transaction })
            .map_err(PhyRxGainExternalBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyRxGainPublishCompletion {
        PhyRxGainPublishCompletion::PbusTimedOut {
            bank: self.bank,
            transaction: self.transaction,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxGainPublishTimerBinding {
    phase: PhyRxGainDelayPhase,
    micros: u32,
}

impl PhyRxGainPublishTimerBinding {
    pub fn new(action: PhyRxGainPublishAction) -> Result<Self, PhyRxGainExternalBindingError> {
        match action {
            PhyRxGainPublishAction::DelayMicros { phase, micros } => Ok(Self { phase, micros }),
            _ => Err(PhyRxGainExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyRxGainPublishCompletion {
        PhyRxGainPublishCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxGainPublishExternalBinding {
    Mmio(PhyRxGainPublishMmioBinding),
    Pbus(PhyRxGainPublishPbusBinding),
    Timer(PhyRxGainPublishTimerBinding),
}

impl PhyRxGainPublishExternalBinding {
    pub fn lower(action: PhyRxGainPublishAction) -> Result<Self, PhyRxGainExternalBindingError> {
        if let Ok(binding) = PhyRxGainPublishMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyRxGainPublishPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyRxGainPublishTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(PhyRxGainExternalBindingError::UnsupportedAction)
    }
}

/// Explicit former `phy_param` inputs of complete
/// `libphy.a[phy_rx_gain.o]::phy_set_rx_gain_table`, size 650.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainInitParameters {
    pub dc_calibrated: bool,
    pub tables_initialized: bool,
    pub dc: PhyRxGainDcParameters,
    pub memory: PhyRxGainMemoryParameters,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainInitOutcome {
    pub dc: Option<PhyRxGainDcOutcome>,
    pub generated_tables: bool,
    pub wifi_last_index: u8,
    pub shared_last_index: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainInitFailure {
    Dc(PhyRxGainDcFailure),
    Publish(PhyRxGainPublishFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainInitAction {
    PrepareDcControlRestore,
    Dc(PhyRxGainDcAction),
    RestoreDcControl,
    Publish(PhyRxGainPublishAction),
    ConfigureLimits { wifi_last_index: u8 },
    EnableIqCorrection,
    Complete,
    Failed(PhyRxGainInitFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainInitCompletion {
    DcControlRestorePrepared,
    Dc(PhyRxGainDcCompletion),
    DcControlRestored,
    Publish(PhyRxGainPublishCompletion),
    LimitsConfigured { wifi_last_index: u8 },
    IqCorrectionEnabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainInitTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "the allocation-free RX-gain owner retains one bounded child transition"
)]
enum InitStep {
    PrepareDcControlRestore,
    Dc(PhyRxGainDcTransition),
    RestoreDcControl { outcome: PhyRxGainDcOutcome },
    RestoreDcControlAfterFailure(PhyRxGainDcFailure),
    Publish(PhyRxGainPublishTransition),
    Limits,
    IqCorrection,
    Complete,
    Failed(PhyRxGainInitFailure),
}

/// Complete heap-free composition of the RX-gain root.
///
/// The reference calibration guard and table-generation guard are explicit
/// owned booleans. A successful DC outcome is held inside this transition
/// until the enclosing `PhyState` commits it; no raw parameter pointer is
/// exposed while hardware calibration is active.
/// The DC child advances in place: it already preserves its full state when
/// rejecting a completion. The root therefore retains coefficient storage
/// across steps instead of copying the child around every accepted action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRxGainInitTransition {
    parameters: Option<PhyRxGainInitParameters>,
    step: InitStep,
    dc_outcome: Option<PhyRxGainDcOutcome>,
    wifi_last_index: u8,
    shared_last_index: u8,
    generated_tables: bool,
}

/// Coarse execution regions; coefficient products remain in the transition.
#[cfg(any(target_arch = "riscv32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Phase {
    Control,
    Dc,
    Publish,
}

impl PhyRxGainInitTransition {
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn direct_dc_parameters(&self) -> Option<PhyRxGainDcParameters> {
        matches!(self.step, InitStep::PrepareDcControlRestore)
            .then(|| self.parameters.map(|parameters| parameters.dc))
            .flatten()
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn commit_direct_dc(
        &mut self,
        result: Result<PhyRxGainDcOutcome, PhyRxGainDcFailure>,
    ) -> Result<(), PhyRxGainInitTransitionError> {
        if !matches!(self.step, InitStep::PrepareDcControlRestore) {
            return Err(PhyRxGainInitTransitionError::WrongCompletion);
        }
        match result {
            Ok(outcome) => {
                let Some(memory) = self.memory_with_dc(outcome) else {
                    return Err(PhyRxGainInitTransitionError::WrongCompletion);
                };
                self.dc_outcome = Some(outcome);
                self.step = InitStep::Publish(PhyRxGainPublishTransition::new(memory));
            }
            Err(failure) => {
                self.step = InitStep::Failed(PhyRxGainInitFailure::Dc(failure));
            }
        }
        Ok(())
    }

    /// Publish both complete RX gain-memory banks and the root tail as one
    /// blocking production transaction. Host verification keeps the granular
    /// publisher transition, but firmware does not route every table entry
    /// through the action/binding ABI.
    #[cfg(target_arch = "riscv32")]
    #[inline(never)]
    pub(crate) fn execute_publish_target_direct<D: crate::executor::wait::PhyShortDelay>(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> Result<bool, crate::target_executor::PhyTargetPortError> {
        let parameters = match &self.step {
            InitStep::Publish(transition) => transition.parameters,
            _ => return Ok(false),
        };
        let mut failure = None;
        for bank in [PhyRxGainBank::Wifi, PhyRxGainBank::Shared] {
            oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
            for index in 0..PBUS_RX_ON_COUNT {
                let transaction = pbus_rx_on(index, parameters.parameter_002);
                if !crate::target_executor::force_pbus_direct(registers, transaction) {
                    failure = Some(PhyRxGainPublishFailure::PbusTimedOut { bank, transaction });
                    break;
                }
            }
            if failure.is_none() {
                oer_esp32s31_hal::phy::pbus::configure_rx_clock(registers, true);
                oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, true);
                let table = generated_table(bank);
                for index in 0..=table.last_index {
                    let entry = phy_generated_rx_gain_memory_entry(&parameters, bank, table, index);
                    oer_esp32s31_hal::phy::memory::program_gain_memory_entry(registers, entry);
                }
                oer_esp32s31_hal::phy::pbus::configure_rx_clock(registers, false);
                oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, false);
                for index in 0..PBUS_RX_ON_COUNT {
                    let transaction = pbus_rx_on(index, parameters.parameter_002);
                    if !crate::target_executor::force_pbus_direct(registers, transaction) {
                        failure = Some(PhyRxGainPublishFailure::PbusTimedOut { bank, transaction });
                        break;
                    }
                }
            }
            if oer_esp32s31_hal::phy::pbus::configure_work_mode(registers) {
                if !D::settle_micros(1) {
                    return Err(
                        crate::target_executor::PhyTargetPortError::HardwareCapabilityUnavailable,
                    );
                }
                oer_esp32s31_hal::phy::agc::configure_pbus_work_mode_pulse(registers);
                if !D::settle_micros(2) {
                    return Err(
                        crate::target_executor::PhyTargetPortError::HardwareCapabilityUnavailable,
                    );
                }
                oer_esp32s31_hal::phy::agc::clear_pbus_work_mode_pulse(registers);
            }
            if failure.is_some() {
                break;
            }
        }

        if let Some(failure) = failure {
            self.step = InitStep::Failed(PhyRxGainInitFailure::Publish(failure));
            return Ok(true);
        }
        self.wifi_last_index = generated_table(PhyRxGainBank::Wifi).last_index;
        self.shared_last_index = generated_table(PhyRxGainBank::Shared).last_index;
        oer_esp32s31_hal::phy::agc::configure_rx_gain_limits(registers, self.wifi_last_index);
        oer_esp32s31_hal::phy::baseband::enable_iq_correction(registers);
        self.step = InitStep::Complete;
        Ok(true)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) fn execute_tail_target_direct(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> bool {
        if !matches!(self.step, InitStep::Limits) {
            return false;
        }
        oer_esp32s31_hal::phy::agc::configure_rx_gain_limits(registers, self.wifi_last_index);
        oer_esp32s31_hal::phy::baseband::enable_iq_correction(registers);
        self.step = InitStep::Complete;
        true
    }

    /// Inspect terminal state without placing the retained coefficient product
    /// in every hot-path action value.
    pub fn terminal(&self) -> Option<Result<PhyRxGainInitOutcome, PhyRxGainInitFailure>> {
        match self.step {
            InitStep::Complete => Some(Ok(PhyRxGainInitOutcome {
                dc: self.dc_outcome,
                generated_tables: self.generated_tables,
                wifi_last_index: self.wifi_last_index,
                shared_last_index: self.shared_last_index,
            })),
            InitStep::Failed(failure) => Some(Err(failure)),
            _ => None,
        }
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn phase(&self) -> Option<Phase> {
        match &self.step {
            InitStep::Complete | InitStep::Failed(_) => None,
            InitStep::Dc(_) => Some(Phase::Dc),
            InitStep::Publish(_) => Some(Phase::Publish),
            _ => Some(Phase::Control),
        }
    }

    #[cfg(test)]
    pub(crate) fn minimum_mut(
        &mut self,
    ) -> Option<&mut crate::rx::dc_offset::PhyRxDcMinimumTransition> {
        match &mut self.step {
            InitStep::Dc(transition) => transition.minimum_mut(),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn finish_minimum(&mut self) -> Result<(), PhyRxGainInitTransitionError> {
        match &mut self.step {
            InitStep::Dc(transition) => transition
                .finish_minimum()
                .map_err(|_| PhyRxGainInitTransitionError::WrongCompletion),
            _ => Err(PhyRxGainInitTransitionError::WrongCompletion),
        }
    }

    /// Lower only the live cursor. Completed coefficient products stay in this
    /// owner until the caller requests the terminal action.
    #[cfg(test)]
    pub(crate) fn prepare_external(
        &self,
    ) -> Result<Option<PhyRxGainInitExternalBinding>, PhyRxGainExternalBindingError> {
        match self.step {
            InitStep::Complete | InitStep::Failed(_) => Ok(None),
            InitStep::Dc(ref transition) => transition
                .lower_external()
                .map(PhyRxGainInitExternalBinding::Dc)
                .map(Some)
                .map_err(|_| PhyRxGainExternalBindingError::UnsupportedAction),
            _ => PhyRxGainInitExternalBinding::lower(self.action()).map(Some),
        }
    }

    pub fn new(parameters: PhyRxGainInitParameters) -> Self {
        let wifi_last_index = generated_table(PhyRxGainBank::Wifi).last_index;
        let shared_last_index = generated_table(PhyRxGainBank::Shared).last_index;
        if parameters.tables_initialized {
            return Self::with_initialized_tables();
        }
        let step = if parameters.dc_calibrated {
            InitStep::Publish(PhyRxGainPublishTransition::new(parameters.memory))
        } else {
            InitStep::PrepareDcControlRestore
        };
        Self {
            parameters: Some(parameters),
            step,
            dc_outcome: None,
            wifi_last_index,
            shared_last_index,
            generated_tables: !parameters.tables_initialized,
        }
    }

    /// Enter the exact vendor tail when both RX calibration products and gain
    /// tables already exist. The tail only republishes hardware limits and IQ
    /// enable state, so it must not materialize or retain the unused coefficient
    /// bank merely to represent this guard result.
    pub fn with_initialized_tables() -> Self {
        Self {
            parameters: None,
            step: InitStep::Limits,
            dc_outcome: None,
            wifi_last_index: generated_table(PhyRxGainBank::Wifi).last_index,
            shared_last_index: generated_table(PhyRxGainBank::Shared).last_index,
            generated_tables: false,
        }
    }

    fn memory_with_dc(&self, outcome: PhyRxGainDcOutcome) -> Option<PhyRxGainMemoryParameters> {
        let mut memory = self.parameters?.memory;
        memory.wifi_index_dc = outcome.wifi_index_dc;
        memory.wifi_fine_dc = outcome.wifi_fine_dc;
        let mut index = 0;
        while index != outcome.shared_index_dc.len() {
            memory.shared_index_dc[index] = outcome.shared_index_dc[index];
            index += 1;
        }
        Some(memory)
    }

    pub fn action(&self) -> PhyRxGainInitAction {
        match self.step {
            InitStep::PrepareDcControlRestore => PhyRxGainInitAction::PrepareDcControlRestore,
            InitStep::Dc(ref transition) => PhyRxGainInitAction::Dc(transition.action()),
            InitStep::RestoreDcControl { .. } | InitStep::RestoreDcControlAfterFailure(_) => {
                PhyRxGainInitAction::RestoreDcControl
            }
            InitStep::Publish(ref transition) => PhyRxGainInitAction::Publish(transition.action()),
            InitStep::Limits => PhyRxGainInitAction::ConfigureLimits {
                wifi_last_index: self.wifi_last_index,
            },
            InitStep::IqCorrection => PhyRxGainInitAction::EnableIqCorrection,
            InitStep::Complete => PhyRxGainInitAction::Complete,
            InitStep::Failed(failure) => PhyRxGainInitAction::Failed(failure),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyRxGainInitCompletion,
    ) -> Result<(), PhyRxGainInitTransitionError> {
        match (&mut self.step, completion) {
            (
                InitStep::PrepareDcControlRestore,
                PhyRxGainInitCompletion::DcControlRestorePrepared,
            ) => {
                let Some(parameters) = self.parameters else {
                    return Err(PhyRxGainInitTransitionError::WrongCompletion);
                };
                self.step = InitStep::Dc(PhyRxGainDcTransition::new(parameters.dc));
            }
            (InitStep::Dc(transition), PhyRxGainInitCompletion::Dc(completion)) => {
                // DC already commits only after its child accepts completion.
                // Keep accumulated coefficients in their existing storage.
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainInitTransitionError::WrongCompletion)?;
                match transition.terminal() {
                    Some(Ok(outcome)) => {
                        self.step = InitStep::RestoreDcControl { outcome };
                    }
                    Some(Err(failure)) => {
                        self.step = InitStep::RestoreDcControlAfterFailure(failure);
                    }
                    _ => {}
                }
            }
            (
                InitStep::RestoreDcControl { outcome },
                PhyRxGainInitCompletion::DcControlRestored,
            ) => {
                let outcome = *outcome;
                let Some(memory) = self.memory_with_dc(outcome) else {
                    return Err(PhyRxGainInitTransitionError::WrongCompletion);
                };
                self.dc_outcome = Some(outcome);
                self.step = InitStep::Publish(PhyRxGainPublishTransition::new(memory));
            }
            (
                InitStep::RestoreDcControlAfterFailure(failure),
                PhyRxGainInitCompletion::DcControlRestored,
            ) => {
                self.step = InitStep::Failed(PhyRxGainInitFailure::Dc(*failure));
            }
            (InitStep::Publish(transition), PhyRxGainInitCompletion::Publish(completion)) => {
                // Only the publisher's small cursor changes per entry. Retain
                // both generated tables in their existing storage.
                transition
                    .advance(completion)
                    .map_err(|_| PhyRxGainInitTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyRxGainPublishAction::Complete(outcome) => {
                        self.wifi_last_index = outcome.wifi_entries - 1;
                        self.shared_last_index = outcome.shared_entries - 1;
                        self.step = InitStep::Limits;
                    }
                    PhyRxGainPublishAction::Failed(failure) => {
                        self.step = InitStep::Failed(PhyRxGainInitFailure::Publish(failure));
                    }
                    _ => {}
                }
            }
            (InitStep::Limits, PhyRxGainInitCompletion::LimitsConfigured { wifi_last_index })
                if wifi_last_index == self.wifi_last_index =>
            {
                self.step = InitStep::IqCorrection;
            }
            (InitStep::IqCorrection, PhyRxGainInitCompletion::IqCorrectionEnabled) => {
                self.step = InitStep::Complete;
            }
            (InitStep::Complete | InitStep::Failed(_), _) => {
                return Err(PhyRxGainInitTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyRxGainInitTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainInitBindingError {
    NotDirectMmio,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRxGainInitRestoreInvariant {
    OwnedByOtherCalibration,
    NestingExceeded,
    NotPending,
}

// Compact operation identity; coefficient products stay in the transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InitMmio {
    PrepareDcControlRestore,
    RestoreDcControl,
    ConfigureLimits { wifi_last_index: u8 },
    EnableIqCorrection,
}

/// Non-cloneable identity token for the direct-MMIO root operations.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyRxGainInitMmioBinding {
    operation: InitMmio,
}

impl PhyRxGainInitMmioBinding {
    pub fn new(action: PhyRxGainInitAction) -> Result<Self, PhyRxGainInitBindingError> {
        let operation = match action {
            PhyRxGainInitAction::PrepareDcControlRestore => InitMmio::PrepareDcControlRestore,
            PhyRxGainInitAction::RestoreDcControl => InitMmio::RestoreDcControl,
            PhyRxGainInitAction::ConfigureLimits { wifi_last_index } => {
                InitMmio::ConfigureLimits { wifi_last_index }
            }
            PhyRxGainInitAction::EnableIqCorrection => InitMmio::EnableIqCorrection,
            _ => return Err(PhyRxGainInitBindingError::NotDirectMmio),
        };
        Ok(Self { operation })
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> PhyRxGainInitAction {
        match self.operation {
            InitMmio::PrepareDcControlRestore => PhyRxGainInitAction::PrepareDcControlRestore,
            InitMmio::RestoreDcControl => PhyRxGainInitAction::RestoreDcControl,
            InitMmio::ConfigureLimits { wifi_last_index } => {
                PhyRxGainInitAction::ConfigureLimits { wifi_last_index }
            }
            InitMmio::EnableIqCorrection => PhyRxGainInitAction::EnableIqCorrection,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<PhyRxGainInitCompletion, PhyRxGainInitRestoreInvariant> {
        match self.operation {
            InitMmio::PrepareDcControlRestore => {
                oer_esp32s31_hal::phy::rx_dco::prepare_control_restore(registers).map_err(
                    |error| match error {
                        oer_esp32s31_hal::types::RxDcoControlPrepareError::RestorePending => {
                            PhyRxGainInitRestoreInvariant::OwnedByOtherCalibration
                        }
                        oer_esp32s31_hal::types::RxDcoControlPrepareError::RestoreStackFull => {
                            PhyRxGainInitRestoreInvariant::NestingExceeded
                        }
                    },
                )?;
                Ok(PhyRxGainInitCompletion::DcControlRestorePrepared)
            }
            InitMmio::RestoreDcControl => {
                oer_esp32s31_hal::phy::rx_dco::restore_control(registers)
                    .map_err(|_| PhyRxGainInitRestoreInvariant::NotPending)?;
                Ok(PhyRxGainInitCompletion::DcControlRestored)
            }
            InitMmio::ConfigureLimits { wifi_last_index } => {
                oer_esp32s31_hal::phy::agc::configure_rx_gain_limits(registers, wifi_last_index);
                Ok(PhyRxGainInitCompletion::LimitsConfigured { wifi_last_index })
            }
            InitMmio::EnableIqCorrection => {
                oer_esp32s31_hal::phy::baseband::enable_iq_correction(registers);
                Ok(PhyRxGainInitCompletion::IqCorrectionEnabled)
            }
        }
    }
}

/// Exhaustive lowering of every non-terminal `phy_set_rx_gain_table` action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyRxGainInitExternalBinding {
    Mmio(PhyRxGainInitMmioBinding),
    Dc(crate::rx::gain_calibration::PhyRxGainDcExternalBinding),
    Publish(PhyRxGainPublishExternalBinding),
}

impl PhyRxGainInitExternalBinding {
    pub fn lower(action: PhyRxGainInitAction) -> Result<Self, PhyRxGainExternalBindingError> {
        match action {
            PhyRxGainInitAction::Dc(action) => {
                crate::rx::gain_calibration::PhyRxGainDcExternalBinding::lower(action)
                    .map(Self::Dc)
                    .map_err(|_| PhyRxGainExternalBindingError::UnsupportedAction)
            }
            PhyRxGainInitAction::Publish(action) => {
                PhyRxGainPublishExternalBinding::lower(action).map(Self::Publish)
            }
            _ => PhyRxGainInitMmioBinding::new(action)
                .map(Self::Mmio)
                .map_err(|_| PhyRxGainExternalBindingError::UnsupportedAction),
        }
    }
}

#[cfg(test)]
mod tests;
