//! Rust-owned TX-DC calibration through the power detector.
//!
//! The pinned esp-phy-lib supplies the 562-byte root
//! `phy_txdc_cal_pwdet_init` and its 948-byte search child
//! `phy_txdc_cal_pwdet_new`. Runtime Wi-Fi and BT/154 use the cleanup-enabled
//! form; vendor diagnostic branches are omitted.
//! The semantic model exposes every delay, SAR observation and PBus force as
//! an externally completed action. The production target executes the same
//! finite graph as one blocking transaction after acquiring exclusive PHY
//! access, matching the vendor execution shape without an async edge or a
//! wide action/completion value inside the hot loop.

use crate::{
    analog::pbus::PhyPbusForceTest,
    tx::calibration::{
        PhyToneSarAction, PhyToneSarCompletion, PhyToneSarFailure, PhyToneSarRequest,
        PhyToneSarTransition,
    },
};

const COMPONENT_COUNT: u8 = 2;
const SCAN_DIRECTION_COUNT: u8 = 2;
const SCAN_POINT_LIMIT: u8 = 50;
const PRECHECK_LIMIT: u8 = 2;
const MEASUREMENT_CAPACITY: usize = 100;
#[cfg(target_arch = "riscv32")]
const DIRECT_OPERATION_LIMIT: u32 = 100_000;
const DEFAULT_DCO: [u16; 4] = [0x100; 4];
const TX_BB_GAIN: [u16; 3] = [0, 0x80, 0x100];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetSearchRequest {
    pub identity: u8,
    pub initial: [u16; 4],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetSearchOutcome {
    pub identity: u8,
    pub dco: [u16; 4],
    pub measurements: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetSearchAction {
    ForcePbus(PhyPbusForceTest),
    DelayMicros {
        identity: u8,
        component: u8,
        measurement: u8,
        micros: u32,
    },
    ToneSar(PhyToneSarAction),
    Complete(PhyTxDcPwdetSearchOutcome),
    Failed(PhyTxDcPwdetSearchFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetSearchCompletion {
    PbusCompleted(PhyPbusForceTest),
    PbusTimedOut(PhyPbusForceTest),
    DelayElapsed {
        identity: u8,
        component: u8,
        measurement: u8,
        micros: u32,
    },
    ToneSar(PhyToneSarCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetSearchFailure {
    PbusTimedOut(PhyPbusForceTest),
    ToneSar(PhyToneSarFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetSearchTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MeasurementKind {
    PrecheckPositive,
    PrecheckNegative,
    Scan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SearchStep {
    ProgramDco {
        transaction: u8,
        kind: MeasurementKind,
    },
    Delay(MeasurementKind),
    Measure {
        kind: MeasurementKind,
        transition: PhyToneSarTransition,
    },
    CommitDco {
        transaction: u8,
    },
    Complete,
    Failed(PhyTxDcPwdetSearchFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetSearchTransition {
    request: PhyTxDcPwdetSearchRequest,
    step: SearchStep,
    working: [u16; 4],
    component: u8,
    precheck: u8,
    positive_step: u8,
    negative_step: u8,
    direction: u8,
    scan_offset: u8,
    boundary: u8,
    minimum: u16,
    samples: [u16; MEASUREMENT_CAPACITY],
    sample_count: u8,
    total_measurements: u8,
    positive_sample: u16,
}

impl PhyTxDcPwdetSearchTransition {
    pub const fn new(request: PhyTxDcPwdetSearchRequest) -> Self {
        let mut working = request.initial;
        working[2] = working[2].wrapping_add(10);
        Self {
            request,
            step: SearchStep::ProgramDco {
                transaction: 0,
                kind: MeasurementKind::PrecheckPositive,
            },
            working,
            component: 0,
            precheck: 0,
            positive_step: 10,
            negative_step: 10,
            direction: 0,
            scan_offset: 0,
            boundary: 0,
            minimum: u16::MAX,
            samples: [0; MEASUREMENT_CAPACITY],
            sample_count: 0,
            total_measurements: 0,
            positive_sample: 0,
        }
    }

    /// Execute the current archive's blocking search shape directly while the
    /// caller retains exclusive physical PHY access. The semantic transition
    /// remains available for host verification, but production does not
    /// materialize an action/completion enum for every PBus write, settle or
    /// SAR status sample.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn execute_target_direct(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
        mut settle: impl FnMut(
            crate::executor::wait::tx::Scope,
            u32,
        ) -> Result<(), crate::target_executor::PhyTargetPortError>,
        mut observe_ready: impl FnMut(bool),
    ) -> Result<
        Result<PhyTxDcPwdetSearchOutcome, PhyTxDcPwdetSearchFailure>,
        crate::target_executor::PhyTargetPortError,
    > {
        use crate::{
            executor::wait::tx::Scope,
            target_executor::{PhyTargetPortError, force_pbus_direct},
        };

        for _ in 0..DIRECT_OPERATION_LIMIT {
            match self.step {
                SearchStep::ProgramDco { transaction, kind } => {
                    let command = self.dco_transaction(transaction);
                    if !force_pbus_direct(registers, command) {
                        return Ok(Err(PhyTxDcPwdetSearchFailure::PbusTimedOut(command)));
                    }
                    self.step = if transaction == 3 {
                        SearchStep::Delay(kind)
                    } else {
                        SearchStep::ProgramDco {
                            transaction: transaction + 1,
                            kind,
                        }
                    };
                }
                SearchStep::Delay(kind) => {
                    settle(Scope::Search, 10)?;
                    let measurement = self
                        .request
                        .identity
                        .wrapping_mul(64)
                        .wrapping_add(self.total_measurements);
                    let mut sum = 0_u32;
                    for sample in 0..2_u8 {
                        crate::hardware::arm_phy_power_detector_tone(registers);
                        settle(Scope::Tone, 1)?;
                        oer_esp32s31_hal::phy::power_detector::trigger_sar(registers);
                        settle(Scope::Sar, 2)?;
                        let mut observations = 0_u16;
                        let ready =
                            crate::executor::wait::poll::bounded::<core::convert::Infallible>(
                                || {
                                    let ready = oer_esp32s31_hal::phy::power_detector::sample_ready(
                                        registers,
                                    );
                                    observe_ready(ready);
                                    observations = observations.wrapping_add(1);
                                    Ok(ready)
                                },
                            )
                            .unwrap_or_else(|never| match never {});
                        if !ready {
                            return Ok(Err(PhyTxDcPwdetSearchFailure::ToneSar(
                                PhyToneSarFailure::ReadyObservationLimit {
                                    measurement,
                                    sample,
                                    observations,
                                },
                            )));
                        }
                        if self.request.clear_tone_after_ready {
                            crate::hardware::clear_phy_power_detector_tone_arm(registers);
                        }
                        sum = sum.wrapping_add(u32::from(
                            oer_esp32s31_hal::phy::power_detector::sample_sar(registers),
                        ));
                    }
                    let measured = (sum / 2) as u16;
                    self.total_measurements = self.total_measurements.wrapping_add(1);
                    match kind {
                        MeasurementKind::PrecheckPositive => {
                            self.positive_sample = measured;
                            self.set_component_delta(-(self.negative_step as i16));
                            self.program(MeasurementKind::PrecheckNegative);
                        }
                        MeasurementKind::PrecheckNegative => self.after_precheck(measured),
                        MeasurementKind::Scan => self.scan_sample(measured),
                    }
                }
                SearchStep::CommitDco { transaction } => {
                    let command = self.dco_transaction(transaction);
                    if !force_pbus_direct(registers, command) {
                        return Ok(Err(PhyTxDcPwdetSearchFailure::PbusTimedOut(command)));
                    }
                    self.step = if transaction == 3 {
                        SearchStep::Complete
                    } else {
                        SearchStep::CommitDco {
                            transaction: transaction + 1,
                        }
                    };
                }
                SearchStep::Complete => {
                    return Ok(Ok(PhyTxDcPwdetSearchOutcome {
                        identity: self.request.identity,
                        dco: self.working,
                        measurements: self.total_measurements,
                    }));
                }
                SearchStep::Failed(failure) => return Ok(Err(failure)),
                SearchStep::Measure { .. } => return Err(PhyTargetPortError::UnexpectedBinding),
            }
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }

    const fn component_index(&self) -> usize {
        2 + self.component as usize
    }

    const fn dco_transaction(&self, transaction: u8) -> PhyPbusForceTest {
        match transaction {
            0 => PhyPbusForceTest::new(2, 1, self.working[0]),
            1 => PhyPbusForceTest::new(3, 1, self.working[1]),
            2 => PhyPbusForceTest::new(2, 2, self.working[2]),
            _ => PhyPbusForceTest::new(3, 2, self.working[3]),
        }
    }

    fn tone_sar(&self) -> PhyToneSarTransition {
        PhyToneSarTransition::new(PhyToneSarRequest {
            measurement: self
                .request
                .identity
                .wrapping_mul(64)
                .wrapping_add(self.total_measurements),
            samples: 2,
            clear_tone_after_ready: self.request.clear_tone_after_ready,
        })
        .expect("two samples are nonzero")
    }

    fn set_component_delta(&mut self, delta: i16) {
        let index = self.component_index();
        self.working[index] = self.request.initial[index].wrapping_add(delta as u16);
    }

    fn program(&mut self, kind: MeasurementKind) {
        self.step = SearchStep::ProgramDco {
            transaction: 0,
            kind,
        };
    }

    fn begin_scan(&mut self) {
        self.direction = 0;
        self.scan_offset = 0;
        self.boundary = 0;
        self.minimum = u16::MAX;
        self.sample_count = 0;
        self.set_component_delta(self.positive_step as i16);
        self.program(MeasurementKind::Scan);
    }

    fn after_precheck(&mut self, negative_sample: u16) {
        if negative_sample.wrapping_add(15) < self.positive_sample {
            self.positive_step = self.positive_step.wrapping_sub(5);
        } else if negative_sample > self.positive_sample.wrapping_add(15) {
            self.negative_step = self.negative_step.wrapping_sub(5);
        } else {
            self.begin_scan();
            return;
        }
        self.precheck += 1;
        if self.precheck == PRECHECK_LIMIT {
            self.begin_scan();
        } else {
            self.set_component_delta(self.positive_step as i16);
            self.program(MeasurementKind::PrecheckPositive);
        }
    }

    fn scan_sample(&mut self, sample: u16) {
        let index = self.sample_count as usize;
        self.samples[index] = sample;
        self.sample_count += 1;
        if sample < self.minimum {
            self.minimum = sample;
        }
        let keep_scanning = (self.scan_offset < 6 || sample <= self.minimum.wrapping_add(30))
            && self.scan_offset + 1 != SCAN_POINT_LIMIT;
        if keep_scanning {
            self.scan_offset += 1;
            let delta = if self.direction == 0 {
                self.positive_step as i16 + self.scan_offset as i16
            } else {
                -(self.negative_step as i16 + self.scan_offset as i16)
            };
            self.set_component_delta(delta);
            self.program(MeasurementKind::Scan);
        } else if self.direction + 1 != SCAN_DIRECTION_COUNT {
            self.boundary = self.scan_offset;
            self.direction = 1;
            self.scan_offset = 0;
            self.minimum = u16::MAX;
            self.set_component_delta(-(self.negative_step as i16));
            self.program(MeasurementKind::Scan);
        } else {
            self.finish_component();
        }
    }

    fn finish_component(&mut self) {
        let count = self.sample_count as usize;
        let boundary = self.boundary as usize;
        let mut index = 0;
        while index != count {
            if index == 0 || index == boundary + 1 {
                if index + 1 < count && self.samples[index + 1] < self.samples[index] {
                    self.samples[index] = self.samples[index + 1];
                }
            } else if self.samples[index] < self.samples[index - 1] {
                self.samples[index] = self.samples[index - 1];
            }
            index += 1;
        }

        let first = self.samples[0];
        let first_negative = self.samples[(boundary + 1).min(count - 1)];
        let threshold = first.abs_diff(first_negative).max(20);
        let minimum = first.min(first_negative);
        let base = self.request.initial[self.component_index()] as i16;
        let mut upper = base.wrapping_add(self.positive_step as i16);
        let low_bound = base.wrapping_sub(self.negative_step as i16);
        let mut lower = low_bound;
        index = 0;
        while index != count {
            if self.samples[index] <= minimum.wrapping_add(threshold) {
                let mut candidate = base
                    .wrapping_add(boundary as i16)
                    .wrapping_add(1)
                    .wrapping_sub(self.negative_step as i16)
                    .wrapping_sub(index as i16);
                if index <= boundary {
                    upper = base
                        .wrapping_add(self.positive_step as i16)
                        .wrapping_add(index as i16);
                    candidate = low_bound;
                }
                lower = candidate;
            }
            index += 1;
        }
        let result = upper.wrapping_add(lower).wrapping_add(1) / 2;
        let component_index = self.component_index();
        self.working[component_index] = result as u16;

        self.component += 1;
        if self.component == COMPONENT_COUNT {
            self.step = SearchStep::CommitDco { transaction: 0 };
        } else {
            // Scan state is reset by `begin_scan` before its first use.
            self.precheck = 0;
            self.positive_step = 10;
            self.negative_step = 10;
            self.set_component_delta(10);
            self.program(MeasurementKind::PrecheckPositive);
        }
    }

    pub const fn action(&self) -> PhyTxDcPwdetSearchAction {
        match self.step {
            SearchStep::ProgramDco { transaction, .. } => {
                PhyTxDcPwdetSearchAction::ForcePbus(self.dco_transaction(transaction))
            }
            SearchStep::Delay(_) => PhyTxDcPwdetSearchAction::DelayMicros {
                identity: self.request.identity,
                component: self.component,
                measurement: self.total_measurements,
                micros: 10,
            },
            SearchStep::Measure { transition, .. } => {
                PhyTxDcPwdetSearchAction::ToneSar(transition.action())
            }
            SearchStep::CommitDco { transaction } => {
                PhyTxDcPwdetSearchAction::ForcePbus(self.dco_transaction(transaction))
            }
            SearchStep::Complete => PhyTxDcPwdetSearchAction::Complete(PhyTxDcPwdetSearchOutcome {
                identity: self.request.identity,
                dco: self.working,
                measurements: self.total_measurements,
            }),
            SearchStep::Failed(failure) => PhyTxDcPwdetSearchAction::Failed(failure),
        }
    }

    pub const fn failure(&self) -> Option<PhyTxDcPwdetSearchFailure> {
        match self.step {
            SearchStep::Failed(failure) => Some(failure),
            _ => None,
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyTxDcPwdetSearchCompletion,
    ) -> Result<(), PhyTxDcPwdetSearchTransitionError> {
        match (self.step, completion) {
            (
                SearchStep::ProgramDco { transaction, kind },
                PhyTxDcPwdetSearchCompletion::PbusCompleted(completed),
            ) if completed == self.dco_transaction(transaction) => {
                self.step = if transaction == 3 {
                    SearchStep::Delay(kind)
                } else {
                    SearchStep::ProgramDco {
                        transaction: transaction + 1,
                        kind,
                    }
                };
            }
            (
                SearchStep::ProgramDco { transaction, .. },
                PhyTxDcPwdetSearchCompletion::PbusTimedOut(completed),
            ) if completed == self.dco_transaction(transaction) => {
                self.step = SearchStep::Failed(PhyTxDcPwdetSearchFailure::PbusTimedOut(completed));
            }
            (
                SearchStep::CommitDco { transaction },
                PhyTxDcPwdetSearchCompletion::PbusCompleted(completed),
            ) if completed == self.dco_transaction(transaction) => {
                self.step = if transaction == 3 {
                    SearchStep::Complete
                } else {
                    SearchStep::CommitDco {
                        transaction: transaction + 1,
                    }
                };
            }
            (
                SearchStep::CommitDco { transaction },
                PhyTxDcPwdetSearchCompletion::PbusTimedOut(completed),
            ) if completed == self.dco_transaction(transaction) => {
                self.step = SearchStep::Failed(PhyTxDcPwdetSearchFailure::PbusTimedOut(completed));
            }
            (
                SearchStep::Delay(kind),
                PhyTxDcPwdetSearchCompletion::DelayElapsed {
                    identity,
                    component,
                    measurement,
                    micros: 10,
                },
            ) if identity == self.request.identity
                && component == self.component
                && measurement == self.total_measurements =>
            {
                self.step = SearchStep::Measure {
                    kind,
                    transition: self.tone_sar(),
                };
            }
            (
                SearchStep::Measure {
                    kind,
                    mut transition,
                },
                PhyTxDcPwdetSearchCompletion::ToneSar(completion),
            ) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxDcPwdetSearchTransitionError::WrongCompletion)?;
                match transition.action() {
                    PhyToneSarAction::Complete(outcome) => {
                        self.total_measurements = self.total_measurements.wrapping_add(1);
                        match kind {
                            MeasurementKind::PrecheckPositive => {
                                self.positive_sample = outcome.sample;
                                self.set_component_delta(-(self.negative_step as i16));
                                self.program(MeasurementKind::PrecheckNegative);
                            }
                            MeasurementKind::PrecheckNegative => {
                                self.after_precheck(outcome.sample);
                            }
                            MeasurementKind::Scan => self.scan_sample(outcome.sample),
                        }
                    }
                    PhyToneSarAction::Failed(failure) => {
                        self.step = SearchStep::Failed(PhyTxDcPwdetSearchFailure::ToneSar(failure));
                    }
                    _ => self.step = SearchStep::Measure { kind, transition },
                }
            }
            (SearchStep::Complete | SearchStep::Failed(_), _) => {
                return Err(PhyTxDcPwdetSearchTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxDcPwdetSearchTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetParameters {
    pub dco: [[u16; 4]; 3],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetOutcome {
    pub dco: [[u16; 4]; 3],
    pub total_measurements: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetFailure {
    PbusTimedOut(PhyPbusForceTest),
    Search(PhyTxDcPwdetSearchFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetDelayPhase {
    InitialTone,
    WorkMode,
    WorkModePulse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetAction {
    PrepareRegisters,
    ConfigureTxClock {
        enabled: bool,
    },
    ConfigurePowerDetector,
    ConfigurePbusDebugMode,
    ReadPbus {
        selector: u8,
        path: u8,
    },
    ForcePbus(PhyPbusForceTest),
    ConfigureTone {
        enabled: bool,
        selector: u16,
        attenuation: u8,
    },
    DelayMicros {
        phase: PhyTxDcPwdetDelayPhase,
        micros: u32,
    },
    ConfigureSarCalibration,
    Search(PhyTxDcPwdetSearchAction),
    ConfigurePbusWorkMode,
    ConfigurePbusWorkModePulse,
    ClearPbusWorkModePulse,
    RestoreRegisters,
    Complete(PhyTxDcPwdetOutcome),
    Failed(PhyTxDcPwdetFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetCompletion {
    RegistersPrepared,
    TxClockConfigured {
        enabled: bool,
    },
    PowerDetectorConfigured,
    PbusDebugModeConfigured,
    PbusRead {
        selector: u8,
        path: u8,
        value: u16,
    },
    PbusCompleted(PhyPbusForceTest),
    PbusTimedOut(PhyPbusForceTest),
    ToneConfigured {
        enabled: bool,
        selector: u16,
        attenuation: u8,
    },
    DelayElapsed {
        phase: PhyTxDcPwdetDelayPhase,
        micros: u32,
    },
    SarCalibrationConfigured,
    Search(PhyTxDcPwdetSearchCompletion),
    PbusWorkModeConfigured {
        settle_required: bool,
    },
    PbusWorkModePulseConfigured,
    PbusWorkModePulseCleared,
    RegistersRestored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RootTerminal {
    Complete,
    Failed(PhyTxDcPwdetFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "the allocation-free calibration root retains one bounded search transition"
)]
enum RootStep {
    Prepare,
    ClockOn,
    PowerDetector,
    Debug,
    TxOff { index: u8 },
    RxOff { index: u8 },
    ToneOn,
    InitialDelay,
    TxOn { index: u8 },
    BluetoothReadForcedPath,
    BluetoothForcePath { value: u16 },
    BluetoothForceTxPath,
    Sar,
    ForceTxPath,
    Gain,
    Search(PhyTxDcPwdetSearchTransition),
    CleanupDco { index: u8, terminal: RootTerminal },
    CleanupTxOff { index: u8, terminal: RootTerminal },
    ToneOff(RootTerminal),
    WorkMode(RootTerminal),
    WorkModeDelay(RootTerminal),
    WorkModePulse(RootTerminal),
    WorkModePulseDelay(RootTerminal),
    WorkModePulseClear(RootTerminal),
    ClockOff(RootTerminal),
    Restore(RootTerminal),
    Complete,
    Failed(PhyTxDcPwdetFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetTransition {
    parameters: PhyTxDcPwdetParameters,
    mode: PhyTxDcPwdetMode,
    step: RootStep,
    dco: [[u16; 4]; 3],
    row: u8,
    total_measurements: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhyTxDcPwdetMode {
    Wifi,
    Bluetooth { tx_path_value: u8 },
}

const fn tx_off(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(4, 1, 0),
        1 => PhyPbusForceTest::new(5, 1, 0),
        2 => PhyPbusForceTest::new(1, 1, 0),
        3 => PhyPbusForceTest::new(1, 2, 0),
        _ => PhyPbusForceTest::new(0, 1, 0),
    }
}

const fn rx_off(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(0, 1, 0),
        1 => PhyPbusForceTest::new(1, 1, 0),
        _ => PhyPbusForceTest::new(1, 2, 0),
    }
}

const fn tx_on(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(0, 1, 0x80),
        1 => PhyPbusForceTest::new(0, 2, 0),
        2 => PhyPbusForceTest::new(4, 2, 0),
        3 => PhyPbusForceTest::new(1, 1, 0x7c),
        4 => PhyPbusForceTest::new(2, 1, 0x100),
        5 => PhyPbusForceTest::new(3, 1, 0x100),
        6 => PhyPbusForceTest::new(2, 2, 0x100),
        7 => PhyPbusForceTest::new(3, 2, 0x100),
        8 => PhyPbusForceTest::new(1, 2, 0),
        9 => PhyPbusForceTest::new(4, 1, 0x0b),
        // Complete ROM phy_pbus_xpd_tx_on(15, 0) ends with selector 5.
        // This precedes SAR setup and the per-row gain selection.
        _ => PhyPbusForceTest::new(5, 1, 0x1cf),
    }
}

const fn default_dco(index: u8) -> PhyPbusForceTest {
    match index {
        0 => PhyPbusForceTest::new(2, 1, DEFAULT_DCO[0]),
        1 => PhyPbusForceTest::new(3, 1, DEFAULT_DCO[1]),
        2 => PhyPbusForceTest::new(2, 2, DEFAULT_DCO[2]),
        _ => PhyPbusForceTest::new(3, 2, DEFAULT_DCO[3]),
    }
}

impl PhyTxDcPwdetTransition {
    pub const fn new(parameters: PhyTxDcPwdetParameters) -> Self {
        Self::new_with_mode(parameters, PhyTxDcPwdetMode::Wifi)
    }

    /// Build the Bluetooth use of archive `phy_txdc_cal_pwdet_init(1, 0, 1)`.
    ///
    /// The common search remains shared, while the Bluetooth-only PBus read
    /// and TX-path force stay explicit in the action graph.
    pub const fn new_bluetooth(parameters: PhyTxDcPwdetParameters, tx_path_value: u8) -> Self {
        Self::new_with_mode(parameters, PhyTxDcPwdetMode::Bluetooth { tx_path_value })
    }

    const fn new_with_mode(parameters: PhyTxDcPwdetParameters, mode: PhyTxDcPwdetMode) -> Self {
        Self {
            parameters,
            mode,
            step: RootStep::Prepare,
            dco: parameters.dco,
            row: 0,
            total_measurements: 0,
        }
    }

    fn cleanup(&mut self, terminal: RootTerminal) {
        self.step = RootStep::CleanupDco { index: 0, terminal };
    }

    fn fail(&mut self, failure: PhyTxDcPwdetFailure) {
        self.cleanup(RootTerminal::Failed(failure));
    }

    const fn outcome(&self) -> PhyTxDcPwdetOutcome {
        PhyTxDcPwdetOutcome {
            dco: self.dco,
            total_measurements: self.total_measurements,
        }
    }

    /// Execute current `phy_txdc_cal_pwdet_init` as one blocking RF
    /// transaction after the supervisor has granted exclusive physical PHY
    /// access. This is the production target path: the semantic transition is
    /// retained for host verification, while per-edge action, binding and
    /// completion values stay out of the calibration hot loop.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn execute_target_direct(
        &mut self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
        mut settle: impl FnMut(
            crate::executor::wait::tx::Scope,
            u32,
        ) -> Result<(), crate::target_executor::PhyTargetPortError>,
        mut observe_ready: impl FnMut(bool),
    ) -> Result<(), crate::target_executor::PhyTargetPortError> {
        use crate::target_executor::{PhyTargetPortError, force_pbus_direct};

        if self.step != RootStep::Prepare {
            return Err(PhyTargetPortError::UnexpectedBinding);
        }
        oer_esp32s31_hal::phy::power_detector::prepare_txdc_calibration(registers)
            .map_err(|_| PhyTargetPortError::HardwareInvariant)?;

        oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, true);
        oer_esp32s31_hal::phy::power_detector::configure_enabled(registers);
        oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);

        let mut terminal = RootTerminal::Complete;
        for index in 0..5 {
            let transaction = tx_off(index);
            if !force_pbus_direct(registers, transaction) {
                terminal = RootTerminal::Failed(PhyTxDcPwdetFailure::PbusTimedOut(transaction));
                break;
            }
        }
        if terminal == RootTerminal::Complete {
            for index in 0..3 {
                let transaction = rx_off(index);
                if !force_pbus_direct(registers, transaction) {
                    terminal = RootTerminal::Failed(PhyTxDcPwdetFailure::PbusTimedOut(transaction));
                    break;
                }
            }
        }
        if terminal == RootTerminal::Complete {
            crate::hardware::configure_phy_calibration_tone_wide(registers, true, 0x200, 0x78);
            settle(crate::executor::wait::tx::Scope::Root, 1)?;
            for index in 0..11 {
                let transaction = tx_on(index);
                if !force_pbus_direct(registers, transaction) {
                    terminal = RootTerminal::Failed(PhyTxDcPwdetFailure::PbusTimedOut(transaction));
                    break;
                }
            }
        }
        if terminal == RootTerminal::Complete
            && let PhyTxDcPwdetMode::Bluetooth { tx_path_value } = self.mode
        {
            let value =
                require_pbus_result(oer_esp32s31_hal::phy::pbus::read_result(registers, 1, 1))
                    .map_err(|_| PhyTargetPortError::HardwareInvariant)?;
            for transaction in [
                PhyPbusForceTest::new(1, 1, value | 2),
                PhyPbusForceTest::new(4, 2, u16::from(tx_path_value) << 3),
            ] {
                if !force_pbus_direct(registers, transaction) {
                    terminal = RootTerminal::Failed(PhyTxDcPwdetFailure::PbusTimedOut(transaction));
                    break;
                }
            }
        }
        if terminal == RootTerminal::Complete {
            oer_esp32s31_hal::phy::power_detector::configure_txdc_sar(registers);
            for row in 0..3_u8 {
                for transaction in [
                    PhyPbusForceTest::new(5, 1, if row == 2 { 0x1e7 } else { 0x1ef }),
                    PhyPbusForceTest::new(1, 2, TX_BB_GAIN[row as usize]),
                ] {
                    if !force_pbus_direct(registers, transaction) {
                        terminal =
                            RootTerminal::Failed(PhyTxDcPwdetFailure::PbusTimedOut(transaction));
                        break;
                    }
                }
                if terminal != RootTerminal::Complete {
                    break;
                }
                let mut search = PhyTxDcPwdetSearchTransition::new(PhyTxDcPwdetSearchRequest {
                    identity: row,
                    initial: self.dco[row as usize],
                    clear_tone_after_ready: self.parameters.clear_tone_after_ready,
                });
                match search.execute_target_direct(registers, &mut settle, &mut observe_ready)? {
                    Ok(outcome) => {
                        self.dco[row as usize] = outcome.dco;
                        self.total_measurements = self
                            .total_measurements
                            .wrapping_add(u16::from(outcome.measurements));
                    }
                    Err(failure) => {
                        terminal = RootTerminal::Failed(PhyTxDcPwdetFailure::Search(failure));
                        break;
                    }
                }
            }
        }

        // The archive cleanup is unconditional after every typed PBus/search
        // failure. Its PBus writes are best effort, but the register restore
        // remains a hard epoch invariant.
        for index in 0..4 {
            let _ = force_pbus_direct(registers, default_dco(index));
        }
        for index in 0..5 {
            let _ = force_pbus_direct(registers, tx_off(index));
        }
        crate::hardware::configure_phy_calibration_tone_wide(registers, false, 0x80, 0x78);
        if oer_esp32s31_hal::phy::pbus::configure_work_mode(registers) {
            settle(crate::executor::wait::tx::Scope::Root, 1)?;
            oer_esp32s31_hal::phy::agc::configure_pbus_work_mode_pulse(registers);
            settle(crate::executor::wait::tx::Scope::Root, 2)?;
            oer_esp32s31_hal::phy::agc::clear_pbus_work_mode_pulse(registers);
        }
        oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, false);
        oer_esp32s31_hal::phy::power_detector::restore_txdc_calibration(registers)
            .map_err(|_| PhyTargetPortError::HardwareInvariant)?;
        self.step = match terminal {
            RootTerminal::Complete => RootStep::Complete,
            RootTerminal::Failed(failure) => RootStep::Failed(failure),
        };
        Ok(())
    }

    pub const fn action(&self) -> PhyTxDcPwdetAction {
        match self.step {
            RootStep::Prepare => PhyTxDcPwdetAction::PrepareRegisters,
            RootStep::ClockOn => PhyTxDcPwdetAction::ConfigureTxClock { enabled: true },
            RootStep::PowerDetector => PhyTxDcPwdetAction::ConfigurePowerDetector,
            RootStep::Debug => PhyTxDcPwdetAction::ConfigurePbusDebugMode,
            RootStep::BluetoothReadForcedPath => PhyTxDcPwdetAction::ReadPbus {
                selector: 1,
                path: 1,
            },
            RootStep::BluetoothForcePath { value } => {
                PhyTxDcPwdetAction::ForcePbus(PhyPbusForceTest::new(1, 1, value | 2))
            }
            RootStep::BluetoothForceTxPath => {
                let PhyTxDcPwdetMode::Bluetooth { tx_path_value } = self.mode else {
                    unreachable!()
                };
                PhyTxDcPwdetAction::ForcePbus(PhyPbusForceTest::new(
                    4,
                    2,
                    (tx_path_value as u16) << 3,
                ))
            }
            RootStep::TxOff { index } | RootStep::CleanupTxOff { index, .. } => {
                PhyTxDcPwdetAction::ForcePbus(tx_off(index))
            }
            RootStep::RxOff { index } => PhyTxDcPwdetAction::ForcePbus(rx_off(index)),
            RootStep::ToneOn => PhyTxDcPwdetAction::ConfigureTone {
                enabled: true,
                selector: 0x200,
                attenuation: 0x78,
            },
            RootStep::InitialDelay => PhyTxDcPwdetAction::DelayMicros {
                phase: PhyTxDcPwdetDelayPhase::InitialTone,
                micros: 1,
            },
            RootStep::TxOn { index } => PhyTxDcPwdetAction::ForcePbus(tx_on(index)),
            RootStep::Sar => PhyTxDcPwdetAction::ConfigureSarCalibration,
            RootStep::ForceTxPath => PhyTxDcPwdetAction::ForcePbus(PhyPbusForceTest::new(
                5,
                1,
                if self.row == 2 { 0x1e7 } else { 0x1ef },
            )),
            RootStep::Gain => PhyTxDcPwdetAction::ForcePbus(PhyPbusForceTest::new(
                1,
                2,
                TX_BB_GAIN[self.row as usize],
            )),
            RootStep::Search(ref transition) => PhyTxDcPwdetAction::Search(transition.action()),
            RootStep::CleanupDco { index, .. } => PhyTxDcPwdetAction::ForcePbus(default_dco(index)),
            RootStep::ToneOff(_) => PhyTxDcPwdetAction::ConfigureTone {
                enabled: false,
                selector: 0x80,
                attenuation: 0x78,
            },
            RootStep::WorkMode(_) => PhyTxDcPwdetAction::ConfigurePbusWorkMode,
            RootStep::WorkModeDelay(_) => PhyTxDcPwdetAction::DelayMicros {
                phase: PhyTxDcPwdetDelayPhase::WorkMode,
                micros: 1,
            },
            RootStep::WorkModePulse(_) => PhyTxDcPwdetAction::ConfigurePbusWorkModePulse,
            RootStep::WorkModePulseDelay(_) => {
                // SOURCE: complete rev0 ROM
                // `esp32s31_rev0_rom.elf::phy_pbus_force_mode(0)`
                // holds the second work-mode pulse for two microseconds. This
                // cleanup is reached by archive `phy_txdc_cal_pwdet_init`.
                PhyTxDcPwdetAction::DelayMicros {
                    phase: PhyTxDcPwdetDelayPhase::WorkModePulse,
                    micros: 2,
                }
            }
            RootStep::WorkModePulseClear(_) => PhyTxDcPwdetAction::ClearPbusWorkModePulse,
            RootStep::ClockOff(_) => PhyTxDcPwdetAction::ConfigureTxClock { enabled: false },
            RootStep::Restore(_) => PhyTxDcPwdetAction::RestoreRegisters,
            RootStep::Complete => PhyTxDcPwdetAction::Complete(self.outcome()),
            RootStep::Failed(failure) => PhyTxDcPwdetAction::Failed(failure),
        }
    }

    fn pbus_completion(
        &mut self,
        completed: PhyPbusForceTest,
        timed_out: bool,
    ) -> Result<(), PhyTxDcPwdetTransitionError> {
        let (expected, next) = match self.step {
            RootStep::TxOff { index } => (
                tx_off(index),
                if index == 4 {
                    RootStep::RxOff { index: 0 }
                } else {
                    RootStep::TxOff { index: index + 1 }
                },
            ),
            RootStep::RxOff { index } => (
                rx_off(index),
                if index == 2 {
                    RootStep::ToneOn
                } else {
                    RootStep::RxOff { index: index + 1 }
                },
            ),
            RootStep::TxOn { index } => (
                tx_on(index),
                if index == 10 {
                    match self.mode {
                        PhyTxDcPwdetMode::Wifi => RootStep::Sar,
                        PhyTxDcPwdetMode::Bluetooth { .. } => RootStep::BluetoothReadForcedPath,
                    }
                } else {
                    RootStep::TxOn { index: index + 1 }
                },
            ),
            RootStep::BluetoothForcePath { value } => (
                PhyPbusForceTest::new(1, 1, value | 2),
                RootStep::BluetoothForceTxPath,
            ),
            RootStep::BluetoothForceTxPath => {
                let PhyTxDcPwdetMode::Bluetooth { tx_path_value } = self.mode else {
                    return Err(PhyTxDcPwdetTransitionError::WrongCompletion);
                };
                (
                    PhyPbusForceTest::new(4, 2, u16::from(tx_path_value) << 3),
                    RootStep::Sar,
                )
            }
            RootStep::ForceTxPath => (
                PhyPbusForceTest::new(5, 1, if self.row == 2 { 0x1e7 } else { 0x1ef }),
                RootStep::Gain,
            ),
            RootStep::Gain => (
                PhyPbusForceTest::new(1, 2, TX_BB_GAIN[self.row as usize]),
                RootStep::Search(PhyTxDcPwdetSearchTransition::new(
                    PhyTxDcPwdetSearchRequest {
                        identity: self.row,
                        initial: self.dco[self.row as usize],
                        clear_tone_after_ready: self.parameters.clear_tone_after_ready,
                    },
                )),
            ),
            RootStep::CleanupDco { index, terminal } => (
                default_dco(index),
                if index == 3 {
                    RootStep::CleanupTxOff { index: 0, terminal }
                } else {
                    RootStep::CleanupDco {
                        index: index + 1,
                        terminal,
                    }
                },
            ),
            RootStep::CleanupTxOff { index, terminal } => (
                tx_off(index),
                if index == 4 {
                    RootStep::ToneOff(terminal)
                } else {
                    RootStep::CleanupTxOff {
                        index: index + 1,
                        terminal,
                    }
                },
            ),
            _ => return Err(PhyTxDcPwdetTransitionError::WrongCompletion),
        };
        if completed != expected {
            return Err(PhyTxDcPwdetTransitionError::WrongCompletion);
        }
        if timed_out {
            match self.step {
                RootStep::CleanupDco { .. } | RootStep::CleanupTxOff { .. } => {
                    // Cleanup is best effort, but retains a previously
                    // selected terminal outcome.
                    self.step = next;
                }
                _ => self.fail(PhyTxDcPwdetFailure::PbusTimedOut(completed)),
            }
        } else {
            self.step = next;
        }
        Ok(())
    }

    pub fn advance(
        &mut self,
        completion: PhyTxDcPwdetCompletion,
    ) -> Result<(), PhyTxDcPwdetTransitionError> {
        match (&mut self.step, completion) {
            (RootStep::Prepare, PhyTxDcPwdetCompletion::RegistersPrepared) => {
                self.step = RootStep::ClockOn;
            }
            (RootStep::ClockOn, PhyTxDcPwdetCompletion::TxClockConfigured { enabled: true }) => {
                self.step = RootStep::PowerDetector
            }
            (RootStep::PowerDetector, PhyTxDcPwdetCompletion::PowerDetectorConfigured) => {
                self.step = RootStep::Debug;
            }
            (RootStep::Debug, PhyTxDcPwdetCompletion::PbusDebugModeConfigured) => {
                self.step = RootStep::TxOff { index: 0 };
            }
            (
                RootStep::ToneOn,
                PhyTxDcPwdetCompletion::ToneConfigured {
                    enabled: true,
                    selector: 0x200,
                    attenuation: 0x78,
                },
            ) => self.step = RootStep::InitialDelay,
            (
                RootStep::InitialDelay,
                PhyTxDcPwdetCompletion::DelayElapsed {
                    phase: PhyTxDcPwdetDelayPhase::InitialTone,
                    micros: 1,
                },
            ) => self.step = RootStep::TxOn { index: 0 },
            (RootStep::Sar, PhyTxDcPwdetCompletion::SarCalibrationConfigured) => {
                self.step = RootStep::ForceTxPath;
            }
            (
                RootStep::BluetoothReadForcedPath,
                PhyTxDcPwdetCompletion::PbusRead {
                    selector: 1,
                    path: 1,
                    value,
                },
            ) => self.step = RootStep::BluetoothForcePath { value },
            (RootStep::Search(transition), PhyTxDcPwdetCompletion::Search(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| PhyTxDcPwdetTransitionError::WrongCompletion)?;
                if let Some(failure) = transition.failure() {
                    self.fail(PhyTxDcPwdetFailure::Search(failure));
                } else if let PhyTxDcPwdetSearchAction::Complete(outcome) = transition.action() {
                    self.dco[self.row as usize] = outcome.dco;
                    self.total_measurements = self
                        .total_measurements
                        .wrapping_add(u16::from(outcome.measurements));
                    self.row += 1;
                    if self.row == 3 {
                        self.cleanup(RootTerminal::Complete);
                    } else {
                        self.step = RootStep::ForceTxPath;
                    }
                }
            }
            (
                RootStep::ToneOff(terminal),
                PhyTxDcPwdetCompletion::ToneConfigured {
                    enabled: false,
                    selector: 0x80,
                    attenuation: 0x78,
                },
            ) => self.step = RootStep::WorkMode(*terminal),
            (
                RootStep::WorkMode(terminal),
                PhyTxDcPwdetCompletion::PbusWorkModeConfigured {
                    settle_required: false,
                },
            ) => self.step = RootStep::ClockOff(*terminal),
            (
                RootStep::WorkMode(terminal),
                PhyTxDcPwdetCompletion::PbusWorkModeConfigured {
                    settle_required: true,
                },
            ) => self.step = RootStep::WorkModeDelay(*terminal),
            (
                RootStep::WorkModeDelay(terminal),
                PhyTxDcPwdetCompletion::DelayElapsed {
                    phase: PhyTxDcPwdetDelayPhase::WorkMode,
                    micros: 1,
                },
            ) => self.step = RootStep::WorkModePulse(*terminal),
            (
                RootStep::WorkModePulse(terminal),
                PhyTxDcPwdetCompletion::PbusWorkModePulseConfigured,
            ) => self.step = RootStep::WorkModePulseDelay(*terminal),
            (
                RootStep::WorkModePulseDelay(terminal),
                PhyTxDcPwdetCompletion::DelayElapsed {
                    phase: PhyTxDcPwdetDelayPhase::WorkModePulse,
                    micros: 2,
                },
            ) => self.step = RootStep::WorkModePulseClear(*terminal),
            (
                RootStep::WorkModePulseClear(terminal),
                PhyTxDcPwdetCompletion::PbusWorkModePulseCleared,
            ) => self.step = RootStep::ClockOff(*terminal),
            (
                RootStep::ClockOff(terminal),
                PhyTxDcPwdetCompletion::TxClockConfigured { enabled: false },
            ) => self.step = RootStep::Restore(*terminal),
            (RootStep::Restore(terminal), PhyTxDcPwdetCompletion::RegistersRestored) => {
                self.step = match *terminal {
                    RootTerminal::Complete => RootStep::Complete,
                    RootTerminal::Failed(failure) => RootStep::Failed(failure),
                };
            }
            (
                RootStep::TxOff { .. }
                | RootStep::RxOff { .. }
                | RootStep::TxOn { .. }
                | RootStep::BluetoothForcePath { .. }
                | RootStep::BluetoothForceTxPath
                | RootStep::ForceTxPath
                | RootStep::Gain
                | RootStep::CleanupDco { .. }
                | RootStep::CleanupTxOff { .. },
                PhyTxDcPwdetCompletion::PbusCompleted(transaction),
            ) => return self.pbus_completion(transaction, false),
            (
                RootStep::TxOff { .. }
                | RootStep::RxOff { .. }
                | RootStep::TxOn { .. }
                | RootStep::BluetoothForcePath { .. }
                | RootStep::BluetoothForceTxPath
                | RootStep::ForceTxPath
                | RootStep::Gain
                | RootStep::CleanupDco { .. }
                | RootStep::CleanupTxOff { .. },
                PhyTxDcPwdetCompletion::PbusTimedOut(transaction),
            ) => return self.pbus_completion(transaction, true),
            (RootStep::Complete | RootStep::Failed(_), _) => {
                return Err(PhyTxDcPwdetTransitionError::AlreadyComplete);
            }
            _ => return Err(PhyTxDcPwdetTransitionError::WrongCompletion),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetBindingError {
    NotDirectMmio,
}

/// A TX-DC hardware binding invariant was violated by target execution.
///
/// This is not a model completion: the outer target runner must poison the
/// unique hardware epoch instead of advancing or attempting ordinary cleanup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetHardwareInvariant {
    /// A second calibration reached prepare before the first restored state.
    RestoreAlreadyPending,
    /// Cleanup reached restore without a successful prepare operation.
    RestoreNotPending,
    /// The transition requested a PBus selector/path without a result window.
    UnsupportedPbusRead,
}

#[cfg(any(target_arch = "riscv32", test))]
fn require_pbus_result(result: Option<u16>) -> Result<u16, PhyTxDcPwdetHardwareInvariant> {
    result.ok_or(PhyTxDcPwdetHardwareInvariant::UnsupportedPbusRead)
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetMmioBinding {
    action: PhyTxDcPwdetAction,
}

impl PhyTxDcPwdetMmioBinding {
    pub fn new(action: PhyTxDcPwdetAction) -> Result<Self, PhyTxDcPwdetBindingError> {
        match action {
            PhyTxDcPwdetAction::PrepareRegisters
            | PhyTxDcPwdetAction::ConfigureTxClock { .. }
            | PhyTxDcPwdetAction::ConfigurePowerDetector
            | PhyTxDcPwdetAction::ConfigurePbusDebugMode
            | PhyTxDcPwdetAction::ReadPbus { .. }
            | PhyTxDcPwdetAction::ConfigureTone { .. }
            | PhyTxDcPwdetAction::ConfigureSarCalibration
            | PhyTxDcPwdetAction::ConfigurePbusWorkMode
            | PhyTxDcPwdetAction::ConfigurePbusWorkModePulse
            | PhyTxDcPwdetAction::ClearPbusWorkModePulse
            | PhyTxDcPwdetAction::RestoreRegisters => Ok(Self { action }),
            _ => Err(PhyTxDcPwdetBindingError::NotDirectMmio),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn execute_target(
        self,
        registers: &mut impl oer_esp32s31_hal::owner::SharedPhyContext,
    ) -> Result<PhyTxDcPwdetCompletion, PhyTxDcPwdetHardwareInvariant> {
        match self.action {
            PhyTxDcPwdetAction::PrepareRegisters => {
                oer_esp32s31_hal::phy::power_detector::prepare_txdc_calibration(registers)
                    .map_err(|_| PhyTxDcPwdetHardwareInvariant::RestoreAlreadyPending)?;
                Ok(PhyTxDcPwdetCompletion::RegistersPrepared)
            }
            PhyTxDcPwdetAction::ConfigureTxClock { enabled } => {
                oer_esp32s31_hal::phy::pbus::configure_tx_clock(registers, enabled);
                Ok(PhyTxDcPwdetCompletion::TxClockConfigured { enabled })
            }
            PhyTxDcPwdetAction::ConfigurePowerDetector => {
                oer_esp32s31_hal::phy::power_detector::configure_enabled(registers);
                Ok(PhyTxDcPwdetCompletion::PowerDetectorConfigured)
            }
            PhyTxDcPwdetAction::ConfigurePbusDebugMode => {
                oer_esp32s31_hal::phy::pbus::configure_debug_mode(registers);
                Ok(PhyTxDcPwdetCompletion::PbusDebugModeConfigured)
            }
            PhyTxDcPwdetAction::ReadPbus { selector, path } => {
                let value = require_pbus_result(oer_esp32s31_hal::phy::pbus::read_result(
                    registers, selector, path,
                ))?;
                Ok(PhyTxDcPwdetCompletion::PbusRead {
                    selector,
                    path,
                    value,
                })
            }
            PhyTxDcPwdetAction::ConfigureTone {
                enabled,
                selector,
                attenuation,
            } => {
                crate::hardware::configure_phy_calibration_tone_wide(
                    registers,
                    enabled,
                    selector,
                    attenuation,
                );
                Ok(PhyTxDcPwdetCompletion::ToneConfigured {
                    enabled,
                    selector,
                    attenuation,
                })
            }
            PhyTxDcPwdetAction::ConfigureSarCalibration => {
                oer_esp32s31_hal::phy::power_detector::configure_txdc_sar(registers);
                Ok(PhyTxDcPwdetCompletion::SarCalibrationConfigured)
            }
            PhyTxDcPwdetAction::ConfigurePbusWorkMode => {
                Ok(PhyTxDcPwdetCompletion::PbusWorkModeConfigured {
                    settle_required: oer_esp32s31_hal::phy::pbus::configure_work_mode(registers),
                })
            }
            PhyTxDcPwdetAction::ConfigurePbusWorkModePulse => {
                oer_esp32s31_hal::phy::agc::configure_pbus_work_mode_pulse(registers);
                Ok(PhyTxDcPwdetCompletion::PbusWorkModePulseConfigured)
            }
            PhyTxDcPwdetAction::ClearPbusWorkModePulse => {
                oer_esp32s31_hal::phy::agc::clear_pbus_work_mode_pulse(registers);
                Ok(PhyTxDcPwdetCompletion::PbusWorkModePulseCleared)
            }
            PhyTxDcPwdetAction::RestoreRegisters => {
                oer_esp32s31_hal::phy::power_detector::restore_txdc_calibration(registers)
                    .map_err(|_| PhyTxDcPwdetHardwareInvariant::RestoreNotPending)?;
                Ok(PhyTxDcPwdetCompletion::RegistersRestored)
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetExternalBindingError {
    UnsupportedAction,
    Pbus(crate::analog::pbus::PhyPbusHardwareBindingError),
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetSearchPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::analog::pbus::PhyPbusHardwareBinding,
}

impl PhyTxDcPwdetSearchPbusBinding {
    pub fn new(action: PhyTxDcPwdetSearchAction) -> Result<Self, PhyTxDcPwdetExternalBindingError> {
        let PhyTxDcPwdetSearchAction::ForcePbus(transaction) = action else {
            return Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction);
        };
        Ok(Self {
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
    ) -> Result<PhyTxDcPwdetSearchCompletion, PhyTxDcPwdetExternalBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyTxDcPwdetSearchCompletion::PbusCompleted)
            .map_err(PhyTxDcPwdetExternalBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyTxDcPwdetSearchCompletion {
        PhyTxDcPwdetSearchCompletion::PbusTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetSearchTimerBinding {
    identity: u8,
    component: u8,
    measurement: u8,
    micros: u32,
}

impl PhyTxDcPwdetSearchTimerBinding {
    pub fn new(action: PhyTxDcPwdetSearchAction) -> Result<Self, PhyTxDcPwdetExternalBindingError> {
        match action {
            PhyTxDcPwdetSearchAction::DelayMicros {
                identity,
                component,
                measurement,
                micros,
            } => Ok(Self {
                identity,
                component,
                measurement,
                micros,
            }),
            _ => Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyTxDcPwdetSearchCompletion {
        PhyTxDcPwdetSearchCompletion::DelayElapsed {
            identity: self.identity,
            component: self.component,
            measurement: self.measurement,
            micros: self.micros,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetSearchExternalBinding {
    Pbus(PhyTxDcPwdetSearchPbusBinding),
    Timer(PhyTxDcPwdetSearchTimerBinding),
    ToneSar(crate::tx::calibration::PhyToneSarExternalBinding),
}

impl PhyTxDcPwdetSearchExternalBinding {
    pub fn lower(
        action: PhyTxDcPwdetSearchAction,
    ) -> Result<Self, PhyTxDcPwdetExternalBindingError> {
        match action {
            PhyTxDcPwdetSearchAction::ForcePbus(_) => {
                PhyTxDcPwdetSearchPbusBinding::new(action).map(Self::Pbus)
            }
            PhyTxDcPwdetSearchAction::DelayMicros { .. } => {
                PhyTxDcPwdetSearchTimerBinding::new(action).map(Self::Timer)
            }
            PhyTxDcPwdetSearchAction::ToneSar(action) => {
                crate::tx::calibration::PhyToneSarExternalBinding::lower(action)
                    .map(Self::ToneSar)
                    .map_err(|_| PhyTxDcPwdetExternalBindingError::UnsupportedAction)
            }
            PhyTxDcPwdetSearchAction::Complete(_) | PhyTxDcPwdetSearchAction::Failed(_) => {
                Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction)
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetPbusBinding {
    transaction: PhyPbusForceTest,
    hardware: crate::analog::pbus::PhyPbusHardwareBinding,
}

impl PhyTxDcPwdetPbusBinding {
    pub fn new(action: PhyTxDcPwdetAction) -> Result<Self, PhyTxDcPwdetExternalBindingError> {
        let PhyTxDcPwdetAction::ForcePbus(transaction) = action else {
            return Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction);
        };
        Ok(Self {
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
    ) -> Result<PhyTxDcPwdetCompletion, PhyTxDcPwdetExternalBindingError> {
        self.hardware
            .into_transaction()
            .map(PhyTxDcPwdetCompletion::PbusCompleted)
            .map_err(PhyTxDcPwdetExternalBindingError::Pbus)
    }

    pub const fn into_timeout_completion(self) -> PhyTxDcPwdetCompletion {
        PhyTxDcPwdetCompletion::PbusTimedOut(self.transaction)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PhyTxDcPwdetTimerBinding {
    phase: PhyTxDcPwdetDelayPhase,
    micros: u32,
}

impl PhyTxDcPwdetTimerBinding {
    pub fn new(action: PhyTxDcPwdetAction) -> Result<Self, PhyTxDcPwdetExternalBindingError> {
        match action {
            PhyTxDcPwdetAction::DelayMicros { phase, micros } => Ok(Self { phase, micros }),
            _ => Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction),
        }
    }

    pub const fn micros(&self) -> u32 {
        self.micros
    }

    pub const fn into_completion(self) -> PhyTxDcPwdetCompletion {
        PhyTxDcPwdetCompletion::DelayElapsed {
            phase: self.phase,
            micros: self.micros,
        }
    }
}

/// Exhaustive lowering of every non-terminal `phy_txdc_cal_pwdet_init` action.
#[derive(Debug, Eq, PartialEq)]
pub enum PhyTxDcPwdetExternalBinding {
    Mmio(PhyTxDcPwdetMmioBinding),
    Pbus(PhyTxDcPwdetPbusBinding),
    Timer(PhyTxDcPwdetTimerBinding),
    Search(PhyTxDcPwdetSearchExternalBinding),
}

impl PhyTxDcPwdetExternalBinding {
    pub fn lower(action: PhyTxDcPwdetAction) -> Result<Self, PhyTxDcPwdetExternalBindingError> {
        if let Ok(binding) = PhyTxDcPwdetMmioBinding::new(action) {
            return Ok(Self::Mmio(binding));
        }
        if let Ok(binding) = PhyTxDcPwdetPbusBinding::new(action) {
            return Ok(Self::Pbus(binding));
        }
        if let Ok(binding) = PhyTxDcPwdetTimerBinding::new(action) {
            return Ok(Self::Timer(binding));
        }
        if let PhyTxDcPwdetAction::Search(action) = action {
            return PhyTxDcPwdetSearchExternalBinding::lower(action).map(Self::Search);
        }
        Err(PhyTxDcPwdetExternalBindingError::UnsupportedAction)
    }
}

#[cfg(test)]
mod tests;
