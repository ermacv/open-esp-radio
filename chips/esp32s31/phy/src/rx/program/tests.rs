use super::*;
use crate::calibration::estimator::{PhyDcIqAccumulatorSnapshot, PhyDcIqReadinessSnapshot};
use std::{format, string::String, vec::Vec};

#[derive(Default)]
pub(crate) struct Oracle {
    pub(crate) trace: Vec<String>,
    pub(crate) measurements: u32,
    pub(crate) scenario: u8,
}
impl Oracle {
    fn record(&mut self, kind: &str, value: impl core::fmt::Debug) {
        self.trace.push(format!("{kind}:{value:?}"));
    }
    pub(crate) fn read_pbus(&self) -> u32 {
        0b001011
    }
    pub(crate) fn estimator(&mut self, action: PhyDcIqAction) -> PhyDcIqCompletion {
        let request = match action {
            PhyDcIqAction::Configure(request)
            | PhyDcIqAction::AwaitReadinessEdge { request, .. } => request,
            _ => panic!("oracle received an estimator outside the atomic capture envelope"),
        };
        self.measurements += 1;
        if self.scenario == 2 && self.measurements % 3 == 1 {
            return PhyDcIqCompletion::CapturePending {
                request,
                snapshot: PhyDcIqReadinessSnapshot {
                    ready: false,
                    activity: true,
                },
            };
        }
        let snapshot = if self.scenario == 0 {
            PhyDcIqAccumulatorSnapshot {
                i: 0,
                q: 0,
                power: 256,
            }
        } else {
            let scale = if self.scenario == 3 {
                1
            } else {
                16 * (i32::from(request.control) + 1)
            };
            PhyDcIqAccumulatorSnapshot {
                i: scale
                    * if self.measurements.is_multiple_of(3) {
                        144
                    } else {
                        -80
                    },
                q: scale
                    * if self.measurements.is_multiple_of(2) {
                        96
                    } else {
                        -64
                    },
                power: if self.scenario == 3 { 1 << 25 } else { 1 << 15 },
            }
        };
        PhyDcIqCompletion::CaptureFinished {
            request,
            accumulators: Some(snapshot),
        }
    }
    pub(crate) fn frequency(&self, action: RfpllFrequencyAction) -> RfpllFrequencyCompletion {
        use RfpllFrequencyAction as A;
        use RfpllFrequencyCompletion as C;
        match action {
            A::StartChannelSwitch {
                frequency_index,
                crystal_selector,
            } => C::ChannelSwitchStarted {
                frequency_index,
                crystal_selector,
            },
            A::ClearChannelSwitch => C::ChannelSwitchCleared,
            A::ReadChannelReady { .. } => C::ChannelReadyObserved { ready: true },
            A::ConfigureNrx { frequency_mhz } => C::NrxConfigured { frequency_mhz },
            A::DelayMicros(micros) => C::DelayElapsed(micros),
            _ => panic!("unexpected frequency action"),
        }
    }
    pub(crate) fn bus(&self, transaction: &mut PhyColdI2cTransaction) {
        loop {
            match transaction.action() {
                PhyColdI2cAction::StartRead { .. } => {
                    transaction.read_started().expect("read starts")
                }
                PhyColdI2cAction::StartWrite { .. } => {
                    transaction.write_started().expect("write starts")
                }
                PhyColdI2cAction::AwaitReadCompletionEdge { .. } => {
                    transaction
                        .observe_read_result(Ok(0xa5))
                        .expect("read completes");
                }
                PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                    transaction
                        .observe_write_result(Ok(()))
                        .expect("write completes");
                }
                PhyColdI2cAction::Complete(_) => return,
            }
        }
    }
    pub(crate) fn step(&mut self, program: &mut RxProgram) -> bool {
        let bus_action = match &program.work {
            Work::Frequency(model) => Some(format!("frequency:{:?}", model.action())),
            Work::Field(model) => Some(format!("field:{:?}", model.action())),
            _ => None,
        };
        match program.step().expect("valid program") {
            RxStep::Complete => return false,
            RxStep::Control(mut step) => {
                self.record("control", step.command());
                step.prepare_publication();
                step.complete(Ok(())).expect("control accepted");
            }
            RxStep::Pbus(mut step) => {
                self.record("pbus", step.command());
                step.prepare_publication();
                step.complete(Ok(true)).expect("pbus accepted");
            }
            RxStep::ReadPbus(step) => {
                self.record("read", step.command());
                step.complete(Ok(self.read_pbus())).expect("read accepted");
            }
            RxStep::Frequency(step) => {
                let action = step.command();
                self.record("frequency", action);
                step.complete(Ok(self.frequency(action)))
                    .expect("frequency accepted");
            }
            RxStep::I2c(mut step) => {
                self.trace.push(bus_action.expect("bus owner"));
                step.prepare_publication();
                self.bus(step.transaction_mut().expect("retained transaction"));
                step.complete(Ok(())).expect("i2c accepted");
            }
            RxStep::GainEntry(step) => {
                self.record("entry", step.command());
                step.complete(Ok(())).expect("entry accepted");
            }
            RxStep::Settle(step) => {
                self.record("settle", step.micros());
                step.complete(Ok(())).expect("settle accepted");
            }
            RxStep::Estimator(step) => {
                let action = step.command();
                self.record("estimate", action);
                let completion = self.estimator(action);
                step.complete(Ok(completion)).expect("estimator accepted");
            }
        }
        true
    }
    pub(crate) fn run(&mut self, program: &mut RxProgram) {
        for _ in 0..100_000 {
            if !self.step(program) {
                return;
            }
        }
        panic!("program exceeded its finite bound");
    }
}

pub(crate) fn parameters() -> PhyRxGainInitParameters {
    PhyRxGainInitParameters {
        dc_calibrated: false,
        tables_initialized: false,
        dc: PhyRxGainDcParameters {
            crystal_selector: 0,
            pbus_rx_path_value: 0xbf,
            rx_saturation_detected: false,
        },
        memory: PhyRxGainMemoryParameters {
            parameter_002: 0xbf,
            wifi_index_dc: [[0x111; 2]; 8],
            wifi_fine_dc: [[0x122; 2]; dc::FINE_CODES],
            shared_index_dc: [[0x133; 2]; 11],
            wifi_auxiliary: 3,
        },
    }
}

#[test]
fn complete_program_keeps_restore_obligations_until_the_real_tail() {
    let mut program = RxProgram::new(parameters());
    let mut oracle = Oracle::default();
    oracle.run(&mut program);
    assert!(matches!(program.terminal(), Some(Ok(_))));
    assert!(
        !program.restore.root
            && !program.restore.calibration
            && !program.restore.registers
            && !program.restore.bus
    );
    assert_eq!(
        oracle
            .trace
            .iter()
            .filter(|s| s.starts_with("entry:"))
            .count(),
        148
    );
}

mod frozen;

fn at_pc(pc: Pc) -> RxProgram {
    let mut program = RxProgram::new(parameters());
    let mut oracle = Oracle::default();
    for _ in 0..100_000 {
        if program.cursor.pc == pc {
            return program;
        }
        assert!(oracle.step(&mut program));
    }
    panic!("requested PC was not reached");
}
pub(crate) fn at_estimator() -> RxProgram {
    at_pc(Pc::Estimate)
}
pub(crate) fn at_field() -> RxProgram {
    at_pc(Pc::SharedI2c)
}

#[test]
#[allow(
    clippy::drop_non_drop,
    reason = "the test explicitly abandons borrowed tokens before publication"
)]
fn dropping_a_token_before_publication_does_not_advance_or_claim_cleanup() {
    let mut program = RxProgram::new(parameters());
    let before = format!("{program:?}");
    let RxStep::Control(step) = program.step().expect("first step") else {
        panic!("first control")
    };
    assert_eq!(step.command(), ControlCommand::PrepareRoot);
    drop(step);
    assert_eq!(format!("{program:?}"), before);
    let mut program = at_pc(Pc::CalI);
    let before = format!("{program:?}");
    let RxStep::Pbus(step) = program.step().expect("pbus step") else {
        panic!("pbus token")
    };
    drop(step);
    assert_eq!(format!("{program:?}"), before);
}

#[test]
fn failed_publication_preserves_the_outer_restore_obligation_and_cannot_finish() {
    let mut program = RxProgram::new(parameters());
    let RxStep::Control(mut step) = program.step().expect("first step") else {
        panic!("first control")
    };
    step.prepare_publication();
    assert_eq!(
        step.complete(Err(RxError::Hardware)),
        Err(RxError::Hardware)
    );
    assert!(program.restore.root);
    assert_eq!(program.fault, Some(RxError::Hardware));
    assert!(program.terminal().is_none());
    assert!(matches!(program.step(), Err(RxError::Hardware)));
}

#[test]
fn a_failed_restore_keeps_the_original_obligation_live() {
    let mut program = at_pc(Pc::RootRestore);
    assert!(program.restore.root);
    let RxStep::Control(step) = program.step().expect("restore") else {
        panic!("restore token")
    };
    assert_eq!(
        step.complete(Err(RxError::Hardware)),
        Err(RxError::Hardware)
    );
    assert!(program.restore.root);
    assert!(program.terminal().is_none());
}

#[test]
fn rejected_estimator_identity_leaves_the_entire_program_unchanged() {
    let mut program = at_estimator();
    let before = format!("{program:?}");
    let RxStep::Estimator(step) = program.step().expect("estimator") else {
        panic!("estimator token")
    };
    let PhyDcIqAction::Configure(mut request) = step.command() else {
        panic!("new estimator")
    };
    request.control ^= 1;
    let wrong = PhyDcIqCompletion::CaptureFinished {
        request,
        accumulators: Some(PhyDcIqAccumulatorSnapshot {
            i: 0,
            q: 0,
            power: 0,
        }),
    };
    assert_eq!(step.complete(Ok(wrong)), Err(RxError::WrongCompletion));
    assert_eq!(format!("{program:?}"), before);
    assert_eq!(program.action_tag(), 21);
}

#[test]
fn false_ready_retains_the_same_bank_gain_and_coefficient_slots() {
    let mut program = at_estimator();
    let bank = program.cursor.bank;
    let gain = program.cursor.gain;
    let memory = program.memory;
    let RxStep::Estimator(step) = program.step().expect("estimator") else {
        panic!("estimator token")
    };
    let PhyDcIqAction::Configure(request) = step.command() else {
        panic!("new estimator")
    };
    step.complete(Ok(PhyDcIqCompletion::CapturePending {
        request,
        snapshot: PhyDcIqReadinessSnapshot {
            ready: false,
            activity: true,
        },
    }))
    .expect("false ready accepted");
    assert_eq!(program.cursor.bank, bank);
    assert_eq!(program.cursor.gain, gain);
    assert_eq!(program.memory, memory);
    let RxStep::Estimator(step) = program.step().expect("same estimator") else {
        panic!("estimator retained")
    };
    assert!(
        matches!(step.command(),PhyDcIqAction::AwaitReadinessEdge {request:r,readiness_samples:1,readiness_activity_edges:1} if r==request)
    );
}

#[test]
fn timeout_finishes_the_estimator_disable_tail_before_calibration_cleanup() {
    let mut program = at_estimator();
    let RxStep::Estimator(step) = program.step().expect("estimator") else {
        panic!("estimator token")
    };
    let PhyDcIqAction::Configure(request) = step.command() else {
        panic!("new estimator")
    };
    step.complete(Ok(PhyDcIqCompletion::CaptureFinished {
        request,
        accumulators: None,
    }))
    .expect("completed disable tail");
    assert_eq!(program.cursor.pc, Pc::CalCleanupI);
    assert!(program.restore.root && program.restore.calibration);
    let mut oracle = Oracle::default();
    oracle.run(&mut program);
    assert!(matches!(
        program.terminal(),
        Some(Err(PhyRxGainInitFailure::Dc(
            PhyRxGainDcFailure::Calibration(PhyRxDcCalibrationFailure::Minimum(
                PhyRxDcMinimumFailure::DcIq(_)
            ))
        )))
    ));
    assert!(!program.restore.root && !program.restore.calibration && !program.restore.registers);
    assert!(!oracle.trace.iter().any(|s| s.starts_with("entry:")));
}

#[test]
fn calibration_failure_cleans_up_with_initial_values_before_outer_restore() {
    let mut program = at_pc(Pc::CalI);
    let Work::Calibration(cal) = &mut program.work else {
        panic!("calibration")
    };
    cal.policy.current = [0x1fe, 1];
    let initial = cal.request.initial;
    let RxStep::Pbus(mut step) = program.step().expect("calibration command") else {
        panic!("pbus")
    };
    let failed = step.command();
    step.prepare_publication();
    step.complete(Ok(false)).expect("semantic timeout");
    let RxStep::Pbus(mut step) = program.step().expect("I cleanup") else {
        panic!("I cleanup")
    };
    assert_eq!(step.command(), PhyPbusForceTest::new(2, 1, initial[0]));
    step.prepare_publication();
    step.complete(Ok(true)).expect("cleanup I");
    let RxStep::Pbus(mut step) = program.step().expect("Q cleanup") else {
        panic!("Q cleanup")
    };
    assert_eq!(step.command(), PhyPbusForceTest::new(3, 1, initial[1]));
    step.prepare_publication();
    step.complete(Ok(true)).expect("cleanup Q");
    assert_eq!(program.cursor.pc, Pc::CalRestore);
    assert!(program.terminal().is_none());
    let mut oracle = Oracle::default();
    oracle.run(&mut program);
    assert_eq!(
        program.terminal(),
        Some(Err(PhyRxGainInitFailure::Dc(
            PhyRxGainDcFailure::Calibration(PhyRxDcCalibrationFailure::PbusForceTimedOut(failed))
        )))
    );
    assert!(!program.restore.root && !program.restore.calibration && !program.restore.registers);
}

#[test]
fn publication_timeout_preserves_identity_and_finishes_work_mode_cleanup() {
    let mut p = parameters();
    p.dc_calibrated = true;
    let mut program = RxProgram::new(p);
    let mut oracle = Oracle::default();
    assert!(oracle.step(&mut program));
    let RxStep::Pbus(mut step) = program.step().expect("publication") else {
        panic!("pbus")
    };
    let command = step.command();
    step.prepare_publication();
    step.complete(Ok(false)).expect("semantic failure");
    assert_eq!(program.cursor.pc, Pc::PublishWorkMode);
    assert!(program.terminal().is_none());
    oracle.run(&mut program);
    assert_eq!(
        program.terminal(),
        Some(Err(PhyRxGainInitFailure::Publish(
            PhyRxGainPublishFailure::PbusTimedOut {
                bank: PhyRxGainBank::Wifi,
                transaction: command
            }
        )))
    );
    assert!(
        oracle
            .trace
            .last()
            .expect("cleanup trace")
            .contains("WorkMode")
    );
    assert!(!oracle.trace.iter().any(|s| s.starts_with("entry:")));
}

#[test]
fn i2c_completion_requires_the_retained_transaction_to_finish() {
    let mut program = at_field();
    let RxStep::I2c(step) = program.step().expect("field") else {
        panic!("field token")
    };
    assert_eq!(step.complete(Ok(())), Err(RxError::WrongCompletion));
    assert_eq!(program.cursor.pc, Pc::SharedI2c);
    assert!(program.bus.is_some());
    let RxStep::I2c(mut step) = program.step().expect("same field") else {
        panic!("field token")
    };
    step.prepare_publication();
    assert_eq!(
        step.complete(Err(RxError::Hardware)),
        Err(RxError::Hardware)
    );
    assert!(
        program.bus.is_some()
            && program.restore.bus
            && program.restore.root
            && program.restore.registers
    );
    assert!(program.terminal().is_none());
}

#[test]
fn minimum_low_and_high_searches_reuse_their_own_previous_slots() {
    for high in [false, true] {
        let mut program = at_pc(Pc::CalSettle);
        program.cursor.high = high;
        let Work::Calibration(cal) = &mut program.work else {
            panic!("calibration")
        };
        let low = crate::calibration::estimator::PhyDcIqEstimate {
            i: 17,
            q: -9,
            power: 3,
        };
        let measured = crate::calibration::estimator::PhyDcIqEstimate {
            i: -13,
            q: 4,
            power: 7,
        };
        cal.policy.low = low;
        cal.policy.measurement = measured;
        let RxStep::Settle(step) = program.step().expect("settle") else {
            panic!("settle token")
        };
        step.complete(Ok(())).expect("minimum starts");
        let Work::Calibration(cal) = &program.work else {
            panic!("calibration retained")
        };
        assert_eq!(
            cal.minimum_request.previous,
            if high { measured } else { low }
        );
        assert_eq!(cal.minimum_request.mode, 1);
    }
}

#[test]
fn later_gains_restart_from_the_first_bank_pair_and_wifi_uses_fine_zero() {
    let mut program = RxProgram::new(parameters());
    program.cursor.gain = 2;
    program.memory.shared_index_dc[0] = [0x121, 0x122];
    program.memory.shared_index_dc[1] = [0x131, 0x132];
    program.memory.wifi_index_dc[0] = [0x101, 0x102];
    program.memory.wifi_index_dc[1] = [0x111, 0x112];
    program.memory.wifi_fine_dc[0] = [0x141, 0x142];
    assert_eq!(program.previous(), [0x121, 0x122]);
    assert_eq!(program.baseband(), [0x100; 2]);
    program.cursor.bank = PhyRxGainDcBank::Wifi;
    assert_eq!(program.previous(), [0x101, 0x102]);
    assert_eq!(program.baseband(), [0x141, 0x142]);
}

#[test]
fn recalibration_discards_stale_fine_coefficients_before_publication() {
    let run = |old| {
        let mut p = parameters();
        p.memory.wifi_fine_dc = [[old; 2]; dc::FINE_CODES];
        let mut program = RxProgram::new(p);
        let mut oracle = Oracle::default();
        oracle.run(&mut program);
        (oracle.trace, program.terminal())
    };
    assert_eq!(run(0), run(19));
}

#[test]
fn initialized_tables_emit_only_limits_and_iq_without_a_calibration_product() {
    let mut program = RxProgram::with_initialized_tables();
    let mut oracle = Oracle::default();
    oracle.run(&mut program);
    assert_eq!(oracle.trace, ["control:Limits(71)", "control:IqCorrection"]);
    let result = program
        .terminal()
        .expect("terminal")
        .expect("successful tail");
    assert!(!result.generated_tables);
    assert!(result.dc.is_none());
}

#[test]
fn radio_final_iteration_publishes_the_codes_actually_forced_and_baseband_exhaustion_restores_initial()
 {
    for stage in [
        PhyRxDcCalibrationStage::Radio,
        PhyRxDcCalibrationStage::Baseband,
    ] {
        let request = PhyRxDcCalibrationRequest {
            shared_radio: false,
            stage,
            control: 0x800,
            initial: [0x100; 2],
            gain_index: 3,
            rx_saturation_detected: false,
        };
        let mut policy = PhyRxDcCalibrationPolicyState::new(request.initial);
        let iterations = if stage == PhyRxDcCalibrationStage::Radio {
            8
        } else {
            12
        };
        for index in 0..iterations {
            let forced = policy.current;
            if stage == PhyRxDcCalibrationStage::Baseband {
                assert!(
                    policy
                        .accept(
                            request,
                            0,
                            3,
                            false,
                            crate::calibration::estimator::PhyDcIqEstimate {
                                i: 0,
                                q: 0,
                                power: 1
                            },
                            0
                        )
                        .is_none()
                );
            }
            let result = policy.accept(
                request,
                0,
                3,
                true,
                crate::calibration::estimator::PhyDcIqEstimate {
                    i: 64,
                    q: -64,
                    power: 1,
                },
                0,
            );
            if index + 1 < iterations {
                assert!(result.is_none());
            } else {
                let result = result.expect("finite calibration");
                assert!(!result.converged);
                assert_eq!(
                    result.configuration,
                    if stage == PhyRxDcCalibrationStage::Radio {
                        forced
                    } else {
                        request.initial
                    }
                );
            }
        }
    }
}

#[test]
fn correction_thresholds_and_shared_mixer_commands_keep_the_reference_policy() {
    assert_eq!(dc::rx_dc_calibration_correction(1, 2, 0), 0);
    assert_eq!(dc::rx_dc_calibration_correction(-1, 2, 0), 0);
    assert_eq!(dc::rx_dc_calibration_correction(24, 2, 3), 3);
    assert_eq!(dc::rx_dc_calibration_correction(-24, 2, 3), -3);
    assert_eq!(
        dc::shared_mixer_dgain_transaction(8, 0xbf),
        PhyPbusForceTest::new(0, 2, 0xb8)
    );
    assert_eq!(
        dc::shared_mixer_dgain_transaction(9, 0xbf),
        PhyPbusForceTest::new(0, 2, 0xbc)
    );
    assert_eq!(
        dc::shared_mixer_dgain_transaction(10, 0xbf),
        PhyPbusForceTest::new(0, 2, 0xbf)
    );
}
