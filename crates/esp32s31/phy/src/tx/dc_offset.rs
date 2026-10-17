//! Rust-owned ESP32-S31 TX-DC calibration.
//!
//! The mandatory Wi-Fi root is `libphy.a[phy_tx_cal.o]::phy_txdc_cal_init`,
//! size 272 bytes. Its current `phy_bb_init` call is fixed to
//! `(&phy_param[0xa8], 15, 0, 0)`. The only algorithmic child outside the
//! archive is rev0 ROM `phy_txdc_cal` at `0x2f82_abbe`, size 476 bytes.
//!
//! Rust owns all five four-halfword results. PBus transactions, two
//! microsecond timer edges, comparator reads and restoration are explicit
//! actions. No completion interrupt for the PAC measurement-ready bit is
//! evidenced, so the readiness poll is retained as one owned sample per
//! issued binding under an outer finite async deadline.

use crate::analog::pbus::PhyPbusForceTest;

pub const PHY_TX_DC_GAIN_COUNT: u8 = 5;
pub const PHY_TX_DC_ITERATION_COUNT: u8 = 12;
const ENTER_PBUS_COUNT: u8 = 11;
const EXIT_PBUS_COUNT: u8 = 7;
const INITIAL_DCO: u16 = 0x100;
const MAX_DCO: u16 = 0x1ff;
const TONE_SELECTOR: u16 = 600;
const TONE_STEP: u8 = 120;
const TX_GAINS: [u16; PHY_TX_DC_GAIN_COUNT as usize] = [0, 128, 256, 32, 160];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcParameters {
    pub pbus_rx_path_value: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcOutcome {
    pub dco: [[u16; 4]; PHY_TX_DC_GAIN_COUNT as usize],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcFailure {
    PbusTimedOut(PhyPbusForceTest),
    ReadyDeadlineElapsed { gain_index: u8, iteration: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcDelayPhase {
    ComparatorSettle { gain_index: u8, iteration: u8 },
    ToneStop,
    PbusWorkMode,
    PbusWorkModePulse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcAction {
    ConfigurePbusDebugMode,
    ReadPbus {
        selector: u8,
        path: u8,
    },
    ForcePbus(PhyPbusForceTest),
    ConfigureTxClock,
    ConfigureTone {
        enabled: bool,
        selector: u16,
        step: u8,
    },
    DelayMicros {
        phase: PhyTxDcDelayPhase,
        micros: u32,
    },
    TriggerMeasurement {
        gain_index: u8,
        iteration: u8,
    },
    PollReady {
        gain_index: u8,
        iteration: u8,
    },
    ReadComparators {
        gain_index: u8,
        iteration: u8,
    },
    ClearMeasurement,
    ConfigurePbusWorkMode,
    ConfigurePbusWorkModePulse,
    ClearPbusWorkModePulse,
    Complete(PhyTxDcOutcome),
    Failed(PhyTxDcFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcCompletion {
    PbusDebugModeConfigured,
    PbusRead {
        selector: u8,
        path: u8,
        value: u16,
    },
    PbusCompleted(PhyPbusForceTest),
    #[cfg_attr(
        not(any(test, feature = "validation-probes")),
        allow(
            dead_code,
            reason = "only the validation binding constructs this timeout"
        )
    )]
    PbusTimedOut(PhyPbusForceTest),
    TxClockConfigured,
    ToneConfigured {
        enabled: bool,
        selector: u16,
        step: u8,
    },
    DelayElapsed {
        phase: PhyTxDcDelayPhase,
        micros: u32,
    },
    MeasurementTriggered {
        gain_index: u8,
        iteration: u8,
    },
    ReadySampled {
        gain_index: u8,
        iteration: u8,
        ready: bool,
    },
    #[cfg_attr(
        not(any(test, feature = "validation-probes")),
        allow(
            dead_code,
            reason = "only the validation binding constructs this timeout"
        )
    )]
    ReadyDeadlineElapsed {
        gain_index: u8,
        iteration: u8,
    },
    ComparatorsRead {
        gain_index: u8,
        iteration: u8,
        comparator_high: [bool; 2],
    },
    MeasurementCleared,
    PbusWorkModeConfigured {
        settle_required: bool,
    },
    PbusWorkModePulseConfigured,
    PbusWorkModePulseCleared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Terminal {
    Complete(PhyTxDcOutcome),
    Failed(PhyTxDcFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Search {
    gain_index: u8,
    iteration: u8,
    i: u16,
    q: u16,
    step: u16,
    i_sum: u16,
    q_sum: u16,
}

impl Search {
    const fn new(gain_index: u8) -> Self {
        Self {
            gain_index,
            iteration: 0,
            i: INITIAL_DCO,
            q: INITIAL_DCO,
            step: 124,
            i_sum: 0,
            q_sum: 0,
        }
    }

    fn apply_comparators(&mut self, comparator_high: [bool; 2]) {
        self.i = adjusted_dco(self.i, self.step, comparator_high[0]);
        self.q = adjusted_dco(self.q, self.step, comparator_high[1]);
        self.step = if self.step == 2 {
            1
        } else {
            (self.step >> 1) + 1
        };
        if self.iteration > 7 {
            self.i_sum = self.i_sum.wrapping_add(self.i);
            self.q_sum = self.q_sum.wrapping_add(self.q);
        }
        self.iteration += 1;
    }

    const fn average(self) -> [u16; 2] {
        [
            self.i_sum.wrapping_add(2) >> 2,
            self.q_sum.wrapping_add(2) >> 2,
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    ConfigurePbusDebugMode,
    EnterPbus { index: u8 },
    ReadBluetoothGainControl,
    ForceBluetoothGainControl { value: u16 },
    ConfigureBluetoothTxPath,
    SelectGain { gain_index: u8 },
    InitializeDco { gain_index: u8, index: u8 },
    ConfigureTxClock { gain_index: u8 },
    ConfigureTone { search: Search, enabled: bool },
    ForceSearchQ(Search),
    ForceSearchI(Search),
    SearchDelay(Search),
    TriggerMeasurement(Search),
    PollReady(Search),
    ReadComparators(Search),
    ForceFinalQ { search: Search, average: [u16; 2] },
    ForceFinalI { search: Search, average: [u16; 2] },
    ClearMeasurement { gain_index: u8 },
    DisableTone { gain_index: u8 },
    ToneStopDelay { gain_index: u8 },
    CleanupTone(Terminal),
    CleanupToneDelay(Terminal),
    ExitPbus { index: u8, terminal: Terminal },
    ConfigurePbusWorkMode(Terminal),
    PbusSettleDelay(Terminal),
    ConfigurePbusWorkModePulse(Terminal),
    PbusPulseDelay(Terminal),
    ClearPbusWorkModePulse(Terminal),
    Complete(PhyTxDcOutcome),
    Failed(PhyTxDcFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyTxDcMode {
    Wifi,
    Bluetooth { tx_path_value: u8 },
}

pub const fn tx_bb_gain(index: u8) -> u16 {
    if index < PHY_TX_DC_GAIN_COUNT {
        TX_GAINS[index as usize]
    } else {
        128
    }
}

pub const fn adjusted_dco(current: u16, step: u16, subtract: bool) -> u16 {
    if subtract {
        current.saturating_sub(step)
    } else {
        let value = current.saturating_add(step);
        if value > MAX_DCO { MAX_DCO } else { value }
    }
}

const fn enter_pbus_transaction(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(0, 1, 0x080),
        1 => PhyPbusForceTest::new(0, 2, 0),
        2 => PhyPbusForceTest::new(4, 2, 0),
        3 => PhyPbusForceTest::new(1, 1, 0x07c),
        4 => PhyPbusForceTest::new(2, 1, 0x100),
        5 => PhyPbusForceTest::new(3, 1, 0x100),
        6 => PhyPbusForceTest::new(2, 2, 0x100),
        7 => PhyPbusForceTest::new(3, 2, 0x100),
        8 => PhyPbusForceTest::new(1, 2, 0),
        9 => PhyPbusForceTest::new(4, 1, 0x00b),
        _ => PhyPbusForceTest::new(5, 1, 0x1cf),
    }
}

const fn exit_pbus_transaction(index: u8, parameters: PhyTxDcParameters) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(4, 2, 1),
        2 => PhyPbusForceTest::new(5, 1, 0),
        3 => PhyPbusForceTest::new(0, 1, 0x040),
        4 => PhyPbusForceTest::new(0, 2, parameters.pbus_rx_path_value as u16),
        5 => PhyPbusForceTest::new(1, 1, 0x189),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

const fn terminal_step(terminal: Terminal) -> Step {
    match terminal {
        Terminal::Complete(outcome) => Step::Complete(outcome),
        Terminal::Failed(failure) => Step::Failed(failure),
    }
}

const fn preserve_failure(terminal: Terminal, transaction: PhyPbusForceTest) -> Terminal {
    match terminal {
        Terminal::Complete(_) => Terminal::Failed(PhyTxDcFailure::PbusTimedOut(transaction)),
        failure => failure,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcTransition {
    parameters: PhyTxDcParameters,
    mode: PhyTxDcMode,
    dco: [[u16; 4]; PHY_TX_DC_GAIN_COUNT as usize],
    step: Step,
}

impl PhyTxDcTransition {
    pub const fn new(parameters: PhyTxDcParameters) -> Self {
        Self {
            parameters,
            mode: PhyTxDcMode::Wifi,
            dco: [[INITIAL_DCO; 4]; PHY_TX_DC_GAIN_COUNT as usize],
            step: Step::ConfigurePbusDebugMode,
        }
    }

    /// Construct the complete archive `phy_bt_txdc_cal_new` graph.
    ///
    /// It reuses the same ROM `phy_txdc_cal` search for the three canonical
    /// Bluetooth baseband gains while retaining the BT-only PBus preparation.
    pub const fn new_bluetooth(parameters: PhyTxDcParameters, tx_path_value: u8) -> Self {
        Self {
            parameters,
            mode: PhyTxDcMode::Bluetooth { tx_path_value },
            dco: [[INITIAL_DCO; 4]; PHY_TX_DC_GAIN_COUNT as usize],
            step: Step::ConfigurePbusDebugMode,
        }
    }

    const fn gain_count(self) -> u8 {
        match self.mode {
            PhyTxDcMode::Wifi => PHY_TX_DC_GAIN_COUNT,
            PhyTxDcMode::Bluetooth { .. } => 3,
        }
    }

    const fn selected_gain(self, gain_index: u8) -> u16 {
        match self.mode {
            PhyTxDcMode::Wifi => tx_bb_gain(gain_index),
            PhyTxDcMode::Bluetooth { .. } => {
                crate::calibration::bluetooth::bluetooth_gain_index_to_baseband(gain_index as u32)
                    as u16
            }
        }
    }

    const fn bluetooth_tx_path_transaction(self) -> PhyPbusForceTest {
        let PhyTxDcMode::Bluetooth { tx_path_value } = self.mode else {
            unreachable!()
        };
        PhyPbusForceTest::new(4, 2, (tx_path_value as u16) << 3)
    }

    pub const fn action(self) -> PhyTxDcAction {
        match self.step {
            Step::ConfigurePbusDebugMode => PhyTxDcAction::ConfigurePbusDebugMode,
            Step::EnterPbus { index } => PhyTxDcAction::ForcePbus(enter_pbus_transaction(index)),
            Step::ReadBluetoothGainControl => PhyTxDcAction::ReadPbus {
                selector: 1,
                path: 1,
            },
            Step::ForceBluetoothGainControl { value } => {
                PhyTxDcAction::ForcePbus(PhyPbusForceTest::new(1, 1, value | 2))
            }
            Step::ConfigureBluetoothTxPath => {
                PhyTxDcAction::ForcePbus(self.bluetooth_tx_path_transaction())
            }
            Step::SelectGain { gain_index } => PhyTxDcAction::ForcePbus(PhyPbusForceTest::new(
                1,
                2,
                self.selected_gain(gain_index),
            )),
            Step::InitializeDco { index, .. } => PhyTxDcAction::ForcePbus(if index == 0 {
                PhyPbusForceTest::new(2, 2, INITIAL_DCO)
            } else {
                PhyPbusForceTest::new(3, 2, INITIAL_DCO)
            }),
            Step::ConfigureTxClock { .. } => PhyTxDcAction::ConfigureTxClock,
            Step::ConfigureTone { enabled, .. } => PhyTxDcAction::ConfigureTone {
                enabled,
                selector: TONE_SELECTOR,
                step: TONE_STEP,
            },
            Step::ForceSearchQ(search) => {
                PhyTxDcAction::ForcePbus(PhyPbusForceTest::new(3, 1, search.q))
            }
            Step::ForceSearchI(search) => {
                PhyTxDcAction::ForcePbus(PhyPbusForceTest::new(2, 1, search.i))
            }
            Step::SearchDelay(search) => PhyTxDcAction::DelayMicros {
                phase: PhyTxDcDelayPhase::ComparatorSettle {
                    gain_index: search.gain_index,
                    iteration: search.iteration,
                },
                micros: 2,
            },
            Step::TriggerMeasurement(search) => PhyTxDcAction::TriggerMeasurement {
                gain_index: search.gain_index,
                iteration: search.iteration,
            },
            Step::PollReady(search) => PhyTxDcAction::PollReady {
                gain_index: search.gain_index,
                iteration: search.iteration,
            },
            Step::ReadComparators(search) => PhyTxDcAction::ReadComparators {
                gain_index: search.gain_index,
                iteration: search.iteration,
            },
            Step::ForceFinalQ { average, .. } => {
                PhyTxDcAction::ForcePbus(PhyPbusForceTest::new(3, 1, average[1]))
            }
            Step::ForceFinalI { average, .. } => {
                PhyTxDcAction::ForcePbus(PhyPbusForceTest::new(2, 1, average[0]))
            }
            Step::ClearMeasurement { .. } => PhyTxDcAction::ClearMeasurement,
            Step::DisableTone { .. } | Step::CleanupTone(_) => PhyTxDcAction::ConfigureTone {
                enabled: false,
                selector: TONE_SELECTOR,
                step: TONE_STEP,
            },
            Step::ToneStopDelay { .. } | Step::CleanupToneDelay(_) => PhyTxDcAction::DelayMicros {
                phase: PhyTxDcDelayPhase::ToneStop,
                micros: 5,
            },
            Step::ExitPbus { index, .. } => {
                PhyTxDcAction::ForcePbus(exit_pbus_transaction(index, self.parameters))
            }
            Step::ConfigurePbusWorkMode(_) => PhyTxDcAction::ConfigurePbusWorkMode,
            Step::PbusSettleDelay(_) => PhyTxDcAction::DelayMicros {
                phase: PhyTxDcDelayPhase::PbusWorkMode,
                micros: 1,
            },
            Step::ConfigurePbusWorkModePulse(_) => PhyTxDcAction::ConfigurePbusWorkModePulse,
            Step::PbusPulseDelay(_) => PhyTxDcAction::DelayMicros {
                phase: PhyTxDcDelayPhase::PbusWorkModePulse,
                micros: 2,
            },
            Step::ClearPbusWorkModePulse(_) => PhyTxDcAction::ClearPbusWorkModePulse,
            Step::Complete(outcome) => PhyTxDcAction::Complete(outcome),
            Step::Failed(failure) => PhyTxDcAction::Failed(failure),
        }
    }

    pub fn advance(&mut self, completion: PhyTxDcCompletion) -> Result<(), PhyTxDcTransitionError> {
        self.step = match (self.step, completion) {
            (Step::ConfigurePbusDebugMode, PhyTxDcCompletion::PbusDebugModeConfigured) => {
                Step::EnterPbus { index: 0 }
            }
            (Step::EnterPbus { index }, PhyTxDcCompletion::PbusCompleted(transaction))
                if transaction == enter_pbus_transaction(index) =>
            {
                if index + 1 == ENTER_PBUS_COUNT {
                    match self.mode {
                        PhyTxDcMode::Wifi => Step::SelectGain { gain_index: 0 },
                        PhyTxDcMode::Bluetooth { .. } => Step::ReadBluetoothGainControl,
                    }
                } else {
                    Step::EnterPbus { index: index + 1 }
                }
            }
            (Step::EnterPbus { index }, PhyTxDcCompletion::PbusTimedOut(transaction))
                if transaction == enter_pbus_transaction(index) =>
            {
                Step::ConfigurePbusWorkMode(Terminal::Failed(PhyTxDcFailure::PbusTimedOut(
                    transaction,
                )))
            }
            (
                Step::ReadBluetoothGainControl,
                PhyTxDcCompletion::PbusRead {
                    selector: 1,
                    path: 1,
                    value,
                },
            ) => Step::ForceBluetoothGainControl { value },
            (
                Step::ForceBluetoothGainControl { value },
                PhyTxDcCompletion::PbusCompleted(transaction),
            ) if transaction == PhyPbusForceTest::new(1, 1, value | 2) => {
                Step::ConfigureBluetoothTxPath
            }
            (
                Step::ForceBluetoothGainControl { value },
                PhyTxDcCompletion::PbusTimedOut(transaction),
            ) if transaction == PhyPbusForceTest::new(1, 1, value | 2) => {
                Step::ConfigurePbusWorkMode(Terminal::Failed(PhyTxDcFailure::PbusTimedOut(
                    transaction,
                )))
            }
            (Step::ConfigureBluetoothTxPath, PhyTxDcCompletion::PbusCompleted(transaction))
                if transaction == self.bluetooth_tx_path_transaction() =>
            {
                Step::SelectGain { gain_index: 0 }
            }
            (Step::ConfigureBluetoothTxPath, PhyTxDcCompletion::PbusTimedOut(transaction))
                if transaction == self.bluetooth_tx_path_transaction() =>
            {
                Step::ConfigurePbusWorkMode(Terminal::Failed(PhyTxDcFailure::PbusTimedOut(
                    transaction,
                )))
            }
            (Step::SelectGain { gain_index }, PhyTxDcCompletion::PbusCompleted(transaction))
                if transaction == PhyPbusForceTest::new(1, 2, self.selected_gain(gain_index)) =>
            {
                Step::InitializeDco {
                    gain_index,
                    index: 0,
                }
            }
            (Step::SelectGain { gain_index }, PhyTxDcCompletion::PbusTimedOut(transaction))
                if transaction == PhyPbusForceTest::new(1, 2, self.selected_gain(gain_index)) =>
            {
                Step::ConfigurePbusWorkMode(Terminal::Failed(PhyTxDcFailure::PbusTimedOut(
                    transaction,
                )))
            }
            (
                Step::InitializeDco { gain_index, index },
                PhyTxDcCompletion::PbusCompleted(transaction),
            ) if transaction
                == if index == 0 {
                    PhyPbusForceTest::new(2, 2, INITIAL_DCO)
                } else {
                    PhyPbusForceTest::new(3, 2, INITIAL_DCO)
                } =>
            {
                if index == 0 {
                    Step::InitializeDco {
                        gain_index,
                        index: 1,
                    }
                } else {
                    Step::ConfigureTxClock { gain_index }
                }
            }
            (Step::InitializeDco { index, .. }, PhyTxDcCompletion::PbusTimedOut(transaction))
                if transaction
                    == if index == 0 {
                        PhyPbusForceTest::new(2, 2, INITIAL_DCO)
                    } else {
                        PhyPbusForceTest::new(3, 2, INITIAL_DCO)
                    } =>
            {
                Step::ConfigurePbusWorkMode(Terminal::Failed(PhyTxDcFailure::PbusTimedOut(
                    transaction,
                )))
            }
            (Step::ConfigureTxClock { gain_index }, PhyTxDcCompletion::TxClockConfigured) => {
                Step::ConfigureTone {
                    search: Search::new(gain_index),
                    enabled: true,
                }
            }
            (
                Step::ConfigureTone {
                    search,
                    enabled: true,
                },
                PhyTxDcCompletion::ToneConfigured {
                    enabled: true,
                    selector: TONE_SELECTOR,
                    step: TONE_STEP,
                },
            ) => Step::ForceSearchQ(search),
            (Step::ForceSearchQ(search), PhyTxDcCompletion::PbusCompleted(transaction))
                if transaction == PhyPbusForceTest::new(3, 1, search.q) =>
            {
                Step::ForceSearchI(search)
            }
            (Step::ForceSearchI(search), PhyTxDcCompletion::PbusCompleted(transaction))
                if transaction == PhyPbusForceTest::new(2, 1, search.i) =>
            {
                Step::SearchDelay(search)
            }
            (
                Step::ForceSearchQ(search) | Step::ForceSearchI(search),
                PhyTxDcCompletion::PbusTimedOut(transaction),
            ) => {
                let expected = match self.step {
                    Step::ForceSearchQ(_) => PhyPbusForceTest::new(3, 1, search.q),
                    _ => PhyPbusForceTest::new(2, 1, search.i),
                };
                if transaction != expected {
                    return Err(PhyTxDcTransitionError::WrongCompletion);
                }
                Step::CleanupTone(Terminal::Failed(PhyTxDcFailure::PbusTimedOut(transaction)))
            }
            (
                Step::SearchDelay(search),
                PhyTxDcCompletion::DelayElapsed {
                    phase:
                        PhyTxDcDelayPhase::ComparatorSettle {
                            gain_index,
                            iteration,
                        },
                    micros: 2,
                },
            ) if gain_index == search.gain_index && iteration == search.iteration => {
                Step::TriggerMeasurement(search)
            }
            (
                Step::TriggerMeasurement(search),
                PhyTxDcCompletion::MeasurementTriggered {
                    gain_index,
                    iteration,
                },
            ) if gain_index == search.gain_index && iteration == search.iteration => {
                Step::PollReady(search)
            }
            (
                Step::PollReady(search),
                PhyTxDcCompletion::ReadySampled {
                    gain_index,
                    iteration,
                    ready,
                },
            ) if gain_index == search.gain_index && iteration == search.iteration => {
                if ready {
                    Step::ReadComparators(search)
                } else {
                    Step::PollReady(search)
                }
            }
            (
                Step::PollReady(search),
                PhyTxDcCompletion::ReadyDeadlineElapsed {
                    gain_index,
                    iteration,
                },
            ) if gain_index == search.gain_index && iteration == search.iteration => {
                Step::CleanupTone(Terminal::Failed(PhyTxDcFailure::ReadyDeadlineElapsed {
                    gain_index,
                    iteration,
                }))
            }
            (
                Step::ReadComparators(mut search),
                PhyTxDcCompletion::ComparatorsRead {
                    gain_index,
                    iteration,
                    comparator_high,
                },
            ) if gain_index == search.gain_index && iteration == search.iteration => {
                search.apply_comparators(comparator_high);
                if search.iteration == PHY_TX_DC_ITERATION_COUNT {
                    let average = search.average();
                    Step::ForceFinalQ { search, average }
                } else {
                    Step::ForceSearchQ(search)
                }
            }
            (
                Step::ForceFinalQ { search, average },
                PhyTxDcCompletion::PbusCompleted(transaction),
            ) if transaction == PhyPbusForceTest::new(3, 1, average[1]) => {
                Step::ForceFinalI { search, average }
            }
            (
                Step::ForceFinalI { search, average },
                PhyTxDcCompletion::PbusCompleted(transaction),
            ) if transaction == PhyPbusForceTest::new(2, 1, average[0]) => {
                self.dco[search.gain_index as usize] =
                    [average[0], average[1], INITIAL_DCO, INITIAL_DCO];
                Step::ClearMeasurement {
                    gain_index: search.gain_index,
                }
            }
            (
                Step::ForceFinalQ { average, .. } | Step::ForceFinalI { average, .. },
                PhyTxDcCompletion::PbusTimedOut(transaction),
            ) => {
                let expected = match self.step {
                    Step::ForceFinalQ { .. } => PhyPbusForceTest::new(3, 1, average[1]),
                    _ => PhyPbusForceTest::new(2, 1, average[0]),
                };
                if transaction != expected {
                    return Err(PhyTxDcTransitionError::WrongCompletion);
                }
                Step::CleanupTone(Terminal::Failed(PhyTxDcFailure::PbusTimedOut(transaction)))
            }
            (Step::ClearMeasurement { gain_index }, PhyTxDcCompletion::MeasurementCleared) => {
                Step::DisableTone { gain_index }
            }
            (
                Step::DisableTone { gain_index },
                PhyTxDcCompletion::ToneConfigured {
                    enabled: false,
                    selector: TONE_SELECTOR,
                    step: TONE_STEP,
                },
            ) => Step::ToneStopDelay { gain_index },
            (
                Step::ToneStopDelay { gain_index },
                PhyTxDcCompletion::DelayElapsed {
                    phase: PhyTxDcDelayPhase::ToneStop,
                    micros: 5,
                },
            ) => {
                if gain_index + 1 == self.gain_count() {
                    Step::ExitPbus {
                        index: 0,
                        terminal: Terminal::Complete(PhyTxDcOutcome { dco: self.dco }),
                    }
                } else {
                    Step::SelectGain {
                        gain_index: gain_index + 1,
                    }
                }
            }
            (
                Step::CleanupTone(terminal),
                PhyTxDcCompletion::ToneConfigured {
                    enabled: false,
                    selector: TONE_SELECTOR,
                    step: TONE_STEP,
                },
            ) => Step::CleanupToneDelay(terminal),
            (
                Step::CleanupToneDelay(terminal),
                PhyTxDcCompletion::DelayElapsed {
                    phase: PhyTxDcDelayPhase::ToneStop,
                    micros: 5,
                },
            ) => Step::ExitPbus { index: 0, terminal },
            (Step::ExitPbus { index, terminal }, PhyTxDcCompletion::PbusCompleted(transaction))
                if transaction == exit_pbus_transaction(index, self.parameters) =>
            {
                if index + 1 == EXIT_PBUS_COUNT {
                    Step::ConfigurePbusWorkMode(terminal)
                } else {
                    Step::ExitPbus {
                        index: index + 1,
                        terminal,
                    }
                }
            }
            (Step::ExitPbus { index, terminal }, PhyTxDcCompletion::PbusTimedOut(transaction))
                if transaction == exit_pbus_transaction(index, self.parameters) =>
            {
                Step::ConfigurePbusWorkMode(preserve_failure(terminal, transaction))
            }
            (
                Step::ConfigurePbusWorkMode(terminal),
                PhyTxDcCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                },
            ) => terminal_step(terminal),
            (
                Step::ConfigurePbusWorkMode(terminal),
                PhyTxDcCompletion::PbusWorkModeConfigured {
                    settle_required: true,
                },
            ) => Step::PbusSettleDelay(terminal),
            (
                Step::PbusSettleDelay(terminal),
                PhyTxDcCompletion::DelayElapsed {
                    phase: PhyTxDcDelayPhase::PbusWorkMode,
                    micros: 1,
                },
            ) => Step::ConfigurePbusWorkModePulse(terminal),
            (
                Step::ConfigurePbusWorkModePulse(terminal),
                PhyTxDcCompletion::PbusWorkModePulseConfigured,
            ) => Step::PbusPulseDelay(terminal),
            (
                Step::PbusPulseDelay(terminal),
                PhyTxDcCompletion::DelayElapsed {
                    phase: PhyTxDcDelayPhase::PbusWorkModePulse,
                    micros: 2,
                },
            ) => Step::ClearPbusWorkModePulse(terminal),
            (
                Step::ClearPbusWorkModePulse(terminal),
                PhyTxDcCompletion::PbusWorkModePulseCleared,
            ) => terminal_step(terminal),
            (Step::Complete(_) | Step::Failed(_), _) => {
                return Err(PhyTxDcTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxDcTransitionError::WrongCompletion),
        };
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcBindingError {
    NotDirectMmio,
    NotReadyPoll,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxDcReadyBinding {
    gain_index: u8,
    iteration: u8,
}

impl PhyTxDcReadyBinding {
    pub fn new(action: PhyTxDcAction) -> Result<Self, PhyTxDcBindingError> {
        match action {
            PhyTxDcAction::PollReady {
                gain_index,
                iteration,
            } => Ok(Self {
                gain_index,
                iteration,
            }),
            _ => Err(PhyTxDcBindingError::NotReadyPoll),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> PhyTxDcCompletion {
        PhyTxDcCompletion::ReadySampled {
            gain_index: self.gain_index,
            iteration: self.iteration,
            ready: crate::hardware::read_phy_tx_dc_ready_status(registers),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxDcMmioBinding {
    action: PhyTxDcAction,
}

impl PhyTxDcMmioBinding {
    pub fn new(action: PhyTxDcAction) -> Result<Self, PhyTxDcBindingError> {
        match action {
            PhyTxDcAction::ConfigurePbusDebugMode
            | PhyTxDcAction::ReadPbus { .. }
            | PhyTxDcAction::ConfigureTxClock
            | PhyTxDcAction::ConfigureTone { .. }
            | PhyTxDcAction::TriggerMeasurement { .. }
            | PhyTxDcAction::ReadComparators { .. }
            | PhyTxDcAction::ClearMeasurement
            | PhyTxDcAction::ConfigurePbusWorkMode
            | PhyTxDcAction::ConfigurePbusWorkModePulse
            | PhyTxDcAction::ClearPbusWorkModePulse => Ok(Self { action }),
            _ => Err(PhyTxDcBindingError::NotDirectMmio),
        }
    }

    pub const fn action(&self) -> PhyTxDcAction {
        self.action
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> PhyTxDcCompletion {
        match self.action {
            PhyTxDcAction::ConfigurePbusDebugMode => {
                oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
                PhyTxDcCompletion::PbusDebugModeConfigured
            }
            PhyTxDcAction::ReadPbus { selector, path } => PhyTxDcCompletion::PbusRead {
                selector,
                path,
                value: {
                    let result =
                        oer_esp32s31_hal::phy::pbus::read_result(registers, selector, path);
                    debug_assert!(
                        result.is_some(),
                        "TX-DC transition emitted an unrecovered PBus selector"
                    );
                    result.unwrap_or(0)
                },
            },
            PhyTxDcAction::ConfigureTxClock => {
                oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, true);
                PhyTxDcCompletion::TxClockConfigured
            }
            PhyTxDcAction::ConfigureTone {
                enabled,
                selector,
                step,
            } => {
                if enabled {
                    // Rev0 ROM `phy_txdc_cal+0x56` calls the original
                    // `phy_start_tx_tone_step`, not the archive `_new`
                    // replacement. The original leaf deliberately leaves DAC
                    // scale and TX-gain compensation disabled while the
                    // comparator search is active.
                    crate::hardware::configure_phy_power_control_tone(registers, selector, step);
                } else {
                    // Preserve the selector/path write performed by
                    // `phy_start_tx_tone_step(0, ...)`, then restore the stop
                    // controls and DAC scale. The surrounding state machine
                    // has already cleared the comparator measurement.
                    crate::hardware::configure_phy_calibration_tone_wide(
                        registers, false, selector, step,
                    );
                    crate::hardware::stop_phy_power_detector_tone(registers);
                }
                PhyTxDcCompletion::ToneConfigured {
                    enabled,
                    selector,
                    step,
                }
            }
            PhyTxDcAction::TriggerMeasurement {
                gain_index,
                iteration,
            } => {
                crate::hardware::trigger_phy_tx_dc_measurement(registers);
                PhyTxDcCompletion::MeasurementTriggered {
                    gain_index,
                    iteration,
                }
            }
            PhyTxDcAction::ReadComparators {
                gain_index,
                iteration,
            } => PhyTxDcCompletion::ComparatorsRead {
                gain_index,
                iteration,
                comparator_high: crate::hardware::read_phy_tx_dc_comparator_status(registers),
            },
            PhyTxDcAction::ClearMeasurement => {
                crate::hardware::clear_phy_tx_dc_measurement(registers);
                PhyTxDcCompletion::MeasurementCleared
            }
            PhyTxDcAction::ConfigurePbusWorkMode => PhyTxDcCompletion::PbusWorkModeConfigured {
                settle_required: oer_esp32s31_hal::phy::pbus::configure_work_mode(registers),
            },
            PhyTxDcAction::ConfigurePbusWorkModePulse => {
                oer_esp32s31_hal::phy::agc::configure_pbus_work_mode_pulse(registers);
                PhyTxDcCompletion::PbusWorkModePulseConfigured
            }
            PhyTxDcAction::ClearPbusWorkModePulse => {
                oer_esp32s31_hal::phy::agc::clear_pbus_work_mode_pulse(registers);
                PhyTxDcCompletion::PbusWorkModePulseCleared
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcExternalBindingError {
    UnsupportedAction,
    Pbus(crate::analog::pbus::PhyPbusHardwareBindingError),
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxDcPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::analog::pbus::PhyPbusHardwareBinding,
}

impl PhyTxDcPbusBinding {
    pub fn new(action: PhyTxDcAction) -> Result<Self, PhyTxDcExternalBindingError> {
        let PhyTxDcAction::ForcePbus(transaction) = action else {
            return Err(PhyTxDcExternalBindingError::UnsupportedAction);
        };
        Ok(Self {
            transaction,
            hardware: crate::analog::pbus::PhyPbusHardwareBinding::new(transaction),
        })
    }

    #[cfg(feature = "validation-probes")]
    pub const fn action(&self) -> crate::analog::pbus::PhyPbusHardwareAction {
        self.hardware.action()
    }

    #[cfg(feature = "validation-probes")]
    pub fn started(&mut self) -> Result<(), crate::analog::pbus::PhyPbusHardwareBindingError> {
        self.hardware.started()
    }

    #[cfg(feature = "validation-probes")]
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

    pub fn into_completion(self) -> Result<PhyTxDcCompletion, PhyTxDcExternalBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyTxDcCompletion::PbusCompleted)
            .map_err(PhyTxDcExternalBindingError::Pbus)
    }

    #[cfg(feature = "validation-probes")]
    pub const fn into_timeout_completion(self) -> PhyTxDcCompletion {
        PhyTxDcCompletion::PbusTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxDcTimerBinding {
    phase: PhyTxDcDelayPhase,
    micros: u32,
}

impl PhyTxDcTimerBinding {
    pub fn new(action: PhyTxDcAction) -> Result<Self, PhyTxDcExternalBindingError> {
        match action {
            PhyTxDcAction::DelayMicros { phase, micros } => Ok(Self { phase, micros }),
            _ => Err(PhyTxDcExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyTxDcCompletion {
        PhyTxDcCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

/// Exhaustive lowering of every non-terminal TX-DC child action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxDcExternalBinding {
    Mmio(PhyTxDcMmioBinding),
    Ready(PhyTxDcReadyBinding),
    Pbus(PhyTxDcPbusBinding),
    Timer(PhyTxDcTimerBinding),
}

impl PhyTxDcExternalBinding {
    pub fn lower(action: PhyTxDcAction) -> Result<Self, PhyTxDcExternalBindingError> {
        if let Ok(binding) = PhyTxDcMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyTxDcReadyBinding::new(action) {
            return Ok(Self::Ready(binding));
        }
        if let Ok(binding) = PhyTxDcPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyTxDcTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        Err(PhyTxDcExternalBindingError::UnsupportedAction)
    }
}

#[cfg(test)]
mod tests;
