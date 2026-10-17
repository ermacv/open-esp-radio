//! Event-driven ESP32-S31 PHY PBus initialization.
//!
//! The rev0 ROM `phy_pbus_clear_reg` body contains twelve calls to
//! `phy_pbus_force_test`. Every call publishes one transaction and then
//! busy-waits on `PHY_PBUS.STATUS_CLOCK_FORCE.BUSY`. The final work-mode path
//! can also execute synchronous one- and two-microsecond delays. This module
//! retains the exact command order while making all readiness and timer edges
//! explicit.

const PHY_PBUS_CLEAR_TRANSACTIONS: [PhyPbusForceTest; 12] = [
    PhyPbusForceTest::new(4, 1, 0),
    PhyPbusForceTest::new(4, 2, 0),
    PhyPbusForceTest::new(5, 1, 0),
    PhyPbusForceTest::new(5, 2, 0),
    PhyPbusForceTest::new(0, 1, 0),
    PhyPbusForceTest::new(0, 2, 0),
    PhyPbusForceTest::new(1, 1, 0),
    PhyPbusForceTest::new(1, 2, 0),
    PhyPbusForceTest::new(2, 1, 0x100),
    PhyPbusForceTest::new(3, 1, 0x100),
    PhyPbusForceTest::new(2, 2, 0x100),
    PhyPbusForceTest::new(3, 2, 0x100),
];

const fn clear_transaction(index: usize) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(4, 2, 0),
        2 => PhyPbusForceTest::new(5, 1, 0),
        3 => PhyPbusForceTest::new(5, 2, 0),
        4 => PhyPbusForceTest::new(0, 1, 0),
        5 => PhyPbusForceTest::new(0, 2, 0),
        6 => PhyPbusForceTest::new(1, 1, 0),
        7 => PhyPbusForceTest::new(1, 2, 0),
        8 => PhyPbusForceTest::new(2, 1, 0x100),
        9 => PhyPbusForceTest::new(3, 1, 0x100),
        10 => PhyPbusForceTest::new(2, 2, 0x100),
        _ => PhyPbusForceTest::new(3, 2, 0x100),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPbusForceTest {
    selector: u8,
    path: u8,
    value: u16,
}

impl PhyPbusForceTest {
    pub const fn new(selector: u8, path: u8, value: u16) -> Self {
        Self {
            selector,
            path,
            value,
        }
    }

    pub const fn selector(self) -> u8 {
        self.selector
    }

    pub const fn path(self) -> u8 {
        self.path
    }

    pub const fn value(self) -> u16 {
        self.value
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusClearAction {
    ConfigureDebugMode,
    ForceTest(PhyPbusForceTest),
    ConfigureWorkMode,
    DelayMicros(u32),
    ConfigureWorkModePulse,
    ClearWorkModePulse,
    Complete(PhyPbusClearOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusClearCompletion {
    DebugModeConfigured,
    ForceTestCompleted(PhyPbusForceTest),
    #[cfg_attr(
        not(any(test, feature = "validation-probes")),
        allow(dead_code, reason = "only validation bindings construct this variant")
    )]
    ForceTestTimedOut(PhyPbusForceTest),
    WorkModeConfigured {
        settle_required: bool,
    },
    DelayElapsed,
    WorkModePulseConfigured,
    WorkModePulseCleared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusClearOutcome {
    Cleared,
    ForceTestTimedOut(PhyPbusForceTest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusClearTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyPbusClearStep {
    DebugMode,
    ForceTest(usize),
    WorkMode,
    SettleDelay,
    WorkModePulse,
    PulseDelay,
    ClearWorkModePulse,
    Complete(PhyPbusClearOutcome),
}

/// Async-capable replacement plan for the complete rev0 ROM
/// `phy_pbus_clear_reg` body at `0x2f82_4572`, size `0x90`.
///
/// A force-test completion must identify the exact transaction at the current
/// cursor. The transition never samples the hardware itself and never
/// advances from a poll; an outer single radio owner performs one start and
/// one completion observation around an independently delivered edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyPbusClearTransition {
    step: PhyPbusClearStep,
}

impl PhyPbusClearTransition {
    pub const fn new() -> Self {
        Self {
            step: PhyPbusClearStep::DebugMode,
        }
    }

    pub const fn action(self) -> PhyPbusClearAction {
        match self.step {
            PhyPbusClearStep::DebugMode => PhyPbusClearAction::ConfigureDebugMode,
            PhyPbusClearStep::ForceTest(index) => {
                PhyPbusClearAction::ForceTest(clear_transaction(index))
            }
            PhyPbusClearStep::WorkMode => PhyPbusClearAction::ConfigureWorkMode,
            PhyPbusClearStep::SettleDelay => PhyPbusClearAction::DelayMicros(1),
            PhyPbusClearStep::WorkModePulse => PhyPbusClearAction::ConfigureWorkModePulse,
            PhyPbusClearStep::PulseDelay => PhyPbusClearAction::DelayMicros(2),
            PhyPbusClearStep::ClearWorkModePulse => PhyPbusClearAction::ClearWorkModePulse,
            PhyPbusClearStep::Complete(outcome) => PhyPbusClearAction::Complete(outcome),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyPbusClearCompletion,
    ) -> Result<(), PhyPbusClearTransitionError> {
        self.step = match (self.step, completion) {
            (PhyPbusClearStep::DebugMode, PhyPbusClearCompletion::DebugModeConfigured) => {
                PhyPbusClearStep::ForceTest(0)
            }
            (
                PhyPbusClearStep::ForceTest(index),
                PhyPbusClearCompletion::ForceTestCompleted(transaction),
            ) if transaction == clear_transaction(index) => {
                if index + 1 == PHY_PBUS_CLEAR_TRANSACTIONS.len() {
                    PhyPbusClearStep::WorkMode
                } else {
                    PhyPbusClearStep::ForceTest(index + 1)
                }
            }
            (
                PhyPbusClearStep::ForceTest(index),
                PhyPbusClearCompletion::ForceTestTimedOut(transaction),
            ) if transaction == clear_transaction(index) => {
                PhyPbusClearStep::Complete(PhyPbusClearOutcome::ForceTestTimedOut(transaction))
            }
            (
                PhyPbusClearStep::WorkMode,
                PhyPbusClearCompletion::WorkModeConfigured {
                    settle_required: false,
                },
            ) => PhyPbusClearStep::Complete(PhyPbusClearOutcome::Cleared),
            (
                PhyPbusClearStep::WorkMode,
                PhyPbusClearCompletion::WorkModeConfigured {
                    settle_required: true,
                },
            ) => PhyPbusClearStep::SettleDelay,
            (PhyPbusClearStep::SettleDelay, PhyPbusClearCompletion::DelayElapsed) => {
                PhyPbusClearStep::WorkModePulse
            }
            (PhyPbusClearStep::WorkModePulse, PhyPbusClearCompletion::WorkModePulseConfigured) => {
                PhyPbusClearStep::PulseDelay
            }
            (PhyPbusClearStep::PulseDelay, PhyPbusClearCompletion::DelayElapsed) => {
                PhyPbusClearStep::ClearWorkModePulse
            }
            (
                PhyPbusClearStep::ClearWorkModePulse,
                PhyPbusClearCompletion::WorkModePulseCleared,
            ) => PhyPbusClearStep::Complete(PhyPbusClearOutcome::Cleared),
            (PhyPbusClearStep::Complete(_), _) => {
                return Err(PhyPbusClearTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyPbusClearTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

impl Default for PhyPbusClearTransition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(any(test, feature = "validation-probes"))]
pub enum PhyPbusHardwareAction {
    Start(PhyPbusForceTest),
    AwaitCompletionEdge(PhyPbusForceTest),
    Complete(PhyPbusForceTest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusHardwareObservation {
    StillPending,
    EdgeConsumed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyPbusHardwareBindingError {
    WrongEdge,
    AlreadyComplete,
    Incomplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyPbusHardwarePhase {
    Start,
    AwaitCompletionEdge,
    Complete,
}

/// Generic non-cloneable owner of one PHY PBus command.
///
/// There is no proven completion interrupt for this block. The executor may
/// therefore submit one command and perform one status observation whenever
/// its Rust timer/deadline wakes it. A busy observation retains ownership and
/// never loops or wakes itself.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyPbusHardwareBinding {
    transaction: PhyPbusForceTest,
    phase: PhyPbusHardwarePhase,
}

impl PhyPbusHardwareBinding {
    pub const fn new(transaction: PhyPbusForceTest) -> Self {
        Self {
            transaction,
            phase: PhyPbusHardwarePhase::Start,
        }
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> PhyPbusHardwareAction {
        match self.phase {
            PhyPbusHardwarePhase::Start => PhyPbusHardwareAction::Start(self.transaction),
            PhyPbusHardwarePhase::AwaitCompletionEdge => {
                PhyPbusHardwareAction::AwaitCompletionEdge(self.transaction)
            }
            PhyPbusHardwarePhase::Complete => PhyPbusHardwareAction::Complete(self.transaction),
        }
    }

    pub fn started(&mut self) -> Result<(), PhyPbusHardwareBindingError> {
        match self.phase {
            PhyPbusHardwarePhase::Start => {
                self.phase = PhyPbusHardwarePhase::AwaitCompletionEdge;
                Ok(())
            }
            PhyPbusHardwarePhase::AwaitCompletionEdge => {
                Err(PhyPbusHardwareBindingError::WrongEdge)
            }
            PhyPbusHardwarePhase::Complete => Err(PhyPbusHardwareBindingError::AlreadyComplete),
        }
    }

    pub fn observe_completed(
        &mut self,
        completed: bool,
    ) -> Result<PhyPbusHardwareObservation, PhyPbusHardwareBindingError> {
        match (self.phase, completed) {
            (PhyPbusHardwarePhase::AwaitCompletionEdge, false) => {
                Ok(PhyPbusHardwareObservation::StillPending)
            }
            (PhyPbusHardwarePhase::AwaitCompletionEdge, true) => {
                self.phase = PhyPbusHardwarePhase::Complete;
                Ok(PhyPbusHardwareObservation::EdgeConsumed)
            }
            (PhyPbusHardwarePhase::Start, _) => Err(PhyPbusHardwareBindingError::WrongEdge),
            (PhyPbusHardwarePhase::Complete, _) => {
                Err(PhyPbusHardwareBindingError::AlreadyComplete)
            }
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn start_target(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<(), PhyPbusHardwareBindingError> {
        if self.phase != PhyPbusHardwarePhase::Start {
            return Err(if self.phase == PhyPbusHardwarePhase::Complete {
                PhyPbusHardwareBindingError::AlreadyComplete
            } else {
                PhyPbusHardwareBindingError::WrongEdge
            });
        }
        oer_esp32s31_hal::phy::pbus::start_force_test(
            registers,
            self.transaction.selector(),
            self.transaction.path(),
            self.transaction.value(),
        );
        self.started()
    }

    #[cfg(target_arch = "riscv32")]
    pub fn observe_target_edge(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> Result<PhyPbusHardwareObservation, PhyPbusHardwareBindingError> {
        if self.phase != PhyPbusHardwarePhase::AwaitCompletionEdge {
            return Err(if self.phase == PhyPbusHardwarePhase::Complete {
                PhyPbusHardwareBindingError::AlreadyComplete
            } else {
                PhyPbusHardwareBindingError::WrongEdge
            });
        }
        match oer_esp32s31_hal::phy::pbus::try_finish_force_test(registers) {
            Ok(()) => self.observe_completed(true),
            Err(oer_esp32s31_hal::phy::pbus::PbusError::Busy) => self.observe_completed(false),
        }
    }

    pub fn into_transaction(self) -> Result<PhyPbusForceTest, PhyPbusHardwareBindingError> {
        if self.phase == PhyPbusHardwarePhase::Complete {
            Ok(self.transaction)
        } else {
            Err(PhyPbusHardwareBindingError::Incomplete)
        }
    }

    pub const fn into_timeout_transaction(self) -> PhyPbusForceTest {
        self.transaction
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyForceTxRxAction {
    Configure {
        enabled: bool,
        phase: u8,
    },
    DelayMicros {
        enabled: bool,
        completed_phase: u8,
        micros: u32,
    },
    Complete {
        enabled: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyForceTxRxCompletion {
    Configured {
        enabled: bool,
        phase: u8,
    },
    DelayElapsed {
        enabled: bool,
        completed_phase: u8,
        micros: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyForceTxRxTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyForceTxRxStep {
    ConfigureFirst,
    DelayFirst,
    ConfigureSecond,
    DelaySecond,
    Complete,
}

/// Nesting count of archive `phy_force_txrx_off_new` held by the callers of
/// one balanced force/release call pair.
///
/// The vendor keeps the count in `phy_param`. Each call changes it by one;
/// a call leaving it at one runs the force sequence, and a call leaving it at
/// zero runs the release sequence. A pair entered at depth zero therefore
/// forces and releases, a pair entered at depth one releases by forcing again,
/// and deeper pairs touch no hardware. OER radio owners are serialized, so the
/// depth of every pair is known from its caller instead of a mutable counter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyForceTxRxDepth(u8);

impl PhyForceTxRxDepth {
    /// No caller holds a force level.
    pub const OUTERMOST: Self = Self(0);

    /// The depth seen by a pair nested inside a pair entered at `self`.
    pub const fn nested(self) -> Self {
        Self(self.0 + 1)
    }

    /// Sequence the pair's first call runs.
    pub const fn enter(self) -> Option<PhyForceTxRxTransition> {
        match self.0 {
            0 => Some(PhyForceTxRxTransition::new(true)),
            _ => None,
        }
    }

    /// Sequence the pair's second call runs, restoring the count to `self`.
    pub const fn leave(self) -> Option<PhyForceTxRxTransition> {
        match self.0 {
            0 => Some(PhyForceTxRxTransition::new(false)),
            1 => Some(PhyForceTxRxTransition::new(true)),
            _ => None,
        }
    }
}

/// Async-capable owner of one force or release sequence: complete rev0 ROM
/// `phy_force_txrx_off`, and the hardware part of archive
/// `phy_force_txrx_off_new` selected by [`PhyForceTxRxDepth`].
///
/// Both branches perform two distinct force-mode writes and retain the
/// one-microsecond delay following each write as an external timer edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyForceTxRxTransition {
    enabled: bool,
    step: PhyForceTxRxStep,
}

impl PhyForceTxRxTransition {
    pub const fn new(enabled: bool) -> Self {
        Self {
            enabled,
            step: PhyForceTxRxStep::ConfigureFirst,
        }
    }

    pub const fn action(self) -> PhyForceTxRxAction {
        match self.step {
            PhyForceTxRxStep::ConfigureFirst => PhyForceTxRxAction::Configure {
                enabled: self.enabled,
                phase: 0,
            },
            PhyForceTxRxStep::DelayFirst => PhyForceTxRxAction::DelayMicros {
                enabled: self.enabled,
                completed_phase: 0,
                micros: 1,
            },
            PhyForceTxRxStep::ConfigureSecond => PhyForceTxRxAction::Configure {
                enabled: self.enabled,
                phase: 1,
            },
            PhyForceTxRxStep::DelaySecond => PhyForceTxRxAction::DelayMicros {
                enabled: self.enabled,
                completed_phase: 1,
                micros: 1,
            },
            PhyForceTxRxStep::Complete => PhyForceTxRxAction::Complete {
                enabled: self.enabled,
            },
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyForceTxRxCompletion,
    ) -> Result<(), PhyForceTxRxTransitionError> {
        self.step = match (self.step, completion) {
            (
                PhyForceTxRxStep::ConfigureFirst,
                PhyForceTxRxCompletion::Configured { enabled, phase: 0 },
            ) if enabled == self.enabled => PhyForceTxRxStep::DelayFirst,
            (
                PhyForceTxRxStep::DelayFirst,
                PhyForceTxRxCompletion::DelayElapsed {
                    enabled,
                    completed_phase: 0,
                    micros: 1,
                },
            ) if enabled == self.enabled => PhyForceTxRxStep::ConfigureSecond,
            (
                PhyForceTxRxStep::ConfigureSecond,
                PhyForceTxRxCompletion::Configured { enabled, phase: 1 },
            ) if enabled == self.enabled => PhyForceTxRxStep::DelaySecond,
            (
                PhyForceTxRxStep::DelaySecond,
                PhyForceTxRxCompletion::DelayElapsed {
                    enabled,
                    completed_phase: 1,
                    micros: 1,
                },
            ) if enabled == self.enabled => PhyForceTxRxStep::Complete,
            (PhyForceTxRxStep::Complete, _) => {
                return Err(PhyForceTxRxTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyForceTxRxTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyForceTxRxBindingError {
    UnsupportedAction,
}

/// Non-cloneable identity for one force-mode register edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyForceTxRxMmioBinding {
    enabled: bool,
    phase: u8,
}

impl PhyForceTxRxMmioBinding {
    pub const fn new(action: PhyForceTxRxAction) -> Result<Self, PhyForceTxRxBindingError> {
        match action {
            PhyForceTxRxAction::Configure { enabled, phase } => Ok(Self { enabled, phase }),
            _ => Err(PhyForceTxRxBindingError::UnsupportedAction),
        }
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> PhyForceTxRxAction {
        PhyForceTxRxAction::Configure {
            enabled: self.enabled,
            phase: self.phase,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    ) -> PhyForceTxRxCompletion {
        oer_esp32s31_hal::phy::pbus::configure_force_txrx(registers, self.enabled, self.phase);
        PhyForceTxRxCompletion::Configured {
            enabled: self.enabled,
            phase: self.phase,
        }
    }
}

/// Consumed identity for one caller-driven one-microsecond timer edge.
#[derive(Debug, Eq, PartialEq)]
pub struct PhyForceTxRxTimerBinding {
    enabled: bool,
    completed_phase: u8,
    micros: u32,
}

impl PhyForceTxRxTimerBinding {
    pub const fn new(action: PhyForceTxRxAction) -> Result<Self, PhyForceTxRxBindingError> {
        match action {
            PhyForceTxRxAction::DelayMicros {
                enabled,
                completed_phase,
                micros,
            } => Ok(Self {
                enabled,
                completed_phase,
                micros,
            }),
            _ => Err(PhyForceTxRxBindingError::UnsupportedAction),
        }
    }

    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn action(&self) -> PhyForceTxRxAction {
        PhyForceTxRxAction::DelayMicros {
            enabled: self.enabled,
            completed_phase: self.completed_phase,
            micros: self.micros,
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyForceTxRxCompletion {
        PhyForceTxRxCompletion::DelayElapsed {
            enabled: self.enabled,
            completed_phase: self.completed_phase,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyForceTxRxExternalBinding {
    Mmio(PhyForceTxRxMmioBinding),
    Timer(PhyForceTxRxTimerBinding),
}

impl PhyForceTxRxExternalBinding {
    pub const fn lower(action: PhyForceTxRxAction) -> Result<Self, PhyForceTxRxBindingError> {
        match action {
            PhyForceTxRxAction::Configure { .. } => match PhyForceTxRxMmioBinding::new(action) {
                Ok(binding) => Ok(Self::Mmio(binding)),
                Err(error) => Err(error),
            },
            PhyForceTxRxAction::DelayMicros { .. } => match PhyForceTxRxTimerBinding::new(action) {
                Ok(binding) => Ok(Self::Timer(binding)),
                Err(error) => Err(error),
            },
            PhyForceTxRxAction::Complete { .. } => Err(PhyForceTxRxBindingError::UnsupportedAction),
        }
    }
}

#[cfg(test)]
mod tests;

pub mod memory;
