use super::*;
use crate::rx::{gain::*, gain_calibration::PhyRxGainDcParameters};

fn publisher() -> PhyRxGainInitTransition {
    PhyRxGainInitTransition::new(PhyRxGainInitParameters {
        dc_calibrated: true,
        tables_initialized: false,
        dc: PhyRxGainDcParameters {
            crystal_selector: 0,
            pbus_rx_path_value: 0xbf,
            rx_saturation_detected: false,
        },
        memory: crate::calibration::baseband::PhyRxGainMemoryParameters {
            parameter_002: 0xbf,
            wifi_index_dc: [[0x101, 0x102]; 8],
            wifi_fine_dc: [[0x103, 0x104]; 5],
            shared_index_dc: [[0x105, 0x106]; 11],
            wifi_auxiliary: 0,
        },
    })
}

#[test]
fn action_identity_preserves_first_entry_flags_across_different_actions() {
    let mut scratch = crate::executor::step::rx_gain::Scratch::default();
    assert_eq!(scratch.observe_action(1), 0x81);
    assert_eq!(scratch.action(), 0x81);
    assert_eq!(scratch.observe_action(33), 0xa1);
    assert_eq!(scratch.observe_action(1), 1);
    assert_eq!(scratch.observe_action(33), 33);
    assert_eq!(scratch.action(), 33);
    assert!(crate::executor::step::rx_gain::Scratch::default().observe_action(1) & 0x80 != 0);
}

#[test]
fn complete_entries_cross_real_poll_boundaries_before_the_root_tail() {
    for measured in [false, true] {
        complete_publisher(measured);
    }
}

fn complete_publisher(measured: bool) {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let mut transition = publisher();
    let mut scratch = crate::executor::step::rx_gain::Scratch::default();
    let mut execution = crate::tracking::observation::RxGainExecution::default();
    let mut timing = EngineTiming::default();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    while transition.phase().is_some() {
        let mut job = Job::rx_gain(
            &mut transition,
            &mut scratch,
            &mut execution,
            clock.now(),
            measured,
        )
        .expect("clock fits");
        let atoms = io.atoms.clone();
        let mut future = core::pin::pin!(drive_calibration::<TestDelay, _, _>(
            &mut job,
            &mut io,
            &clock,
            measured.then_some(&mut timing)
        ));
        loop {
            let before = atoms.get();
            let result = future.as_mut().poll(&mut cx);
            assert!(
                atoms.get() <= before + 1,
                "at most one admitted atom per poll"
            );
            if let Poll::Ready(result) = result {
                result.expect("publisher succeeds");
                break;
            }
            clock.advance(Duration::from_micros(1)).expect("clock fits");
        }
    }
    let Some(Ok(outcome)) = transition.terminal() else {
        panic!("both banks and tail must complete");
    };
    assert!(outcome.generated_tables);
    if measured {
        assert!(timing.polls > 0);
        assert!(timing.ready_edges > 0);
    }
    assert_eq!(
        outcome.dc, None,
        "the pre-calibrated path does not measure again"
    );
    assert_eq!(
        io.power_gain_entries.len(),
        usize::from(outcome.wifi_last_index) + usize::from(outcome.shared_last_index) + 2
    );
}

#[test]
fn dropped_publication_does_not_finish_or_run_another_entry() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let mut transition = publisher();
    let mut scratch = crate::executor::step::rx_gain::Scratch::default();
    let mut execution = crate::tracking::observation::RxGainExecution::default();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut job = Job::rx_gain(
            &mut transition,
            &mut scratch,
            &mut execution,
            clock.now(),
            false,
        )
        .expect("clock fits");
        let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job, &mut io, &clock, None, None, None
        ));
        assert!(future.as_mut().poll(&mut cx).is_pending());
    }
    assert!(transition.terminal().is_none());
    assert!(io.power_gain_entries.is_empty());
    assert_eq!(io.atoms.get(), 1);
}

fn dc_parameters() -> PhyRxGainInitParameters {
    PhyRxGainInitParameters {
        dc_calibrated: false,
        tables_initialized: false,
        dc: PhyRxGainDcParameters {
            crystal_selector: 0,
            pbus_rx_path_value: 0xbf,
            rx_saturation_detected: false,
        },
        memory: crate::calibration::baseband::PhyRxGainMemoryParameters {
            parameter_002: 0xbf,
            wifi_index_dc: [[0; 2]; 8],
            wifi_fine_dc: [[0; 2]; 5],
            shared_index_dc: [[0; 2]; 11],
            wifi_auxiliary: 0,
        },
    }
}

fn root_at_i2c() -> PhyRxGainInitTransition {
    root_at_command(false)
}

fn root_at_command(calibration: bool) -> PhyRxGainInitTransition {
    use crate::analog::rfpll::{RfpllFrequencyAction as R, RfpllFrequencyCompletion as RC};
    use crate::rx::gain_calibration::{PhyRxGainDcAction, PhyRxGainDcCompletion};
    use oer_espressif_analog::Action as I;
    use oer_espressif_analog::Completion as IC;
    let mut root = PhyRxGainInitTransition::new(dc_parameters());
    for _ in 0..100 {
        if !calibration
            && matches!(
                root.action(),
                PhyRxGainInitAction::Dc(PhyRxGainDcAction::I2c(_))
            )
        {
            return root;
        }
        if calibration
            && matches!(
                root.action(),
                PhyRxGainInitAction::Dc(PhyRxGainDcAction::Calibration(
                    crate::rx::gain_calibration::PhyRxDcCalibrationAction::ForcePbus(_)
                ))
            )
        {
            return root;
        }
        let before = root;
        assert_eq!(
            root.finish_minimum(),
            Err(PhyRxGainInitTransitionError::WrongCompletion)
        );
        assert_eq!(root, before);
        let completion = match root.action() {
            PhyRxGainInitAction::PrepareDcControlRestore => {
                PhyRxGainInitCompletion::DcControlRestorePrepared
            }
            PhyRxGainInitAction::Dc(action) => PhyRxGainInitCompletion::Dc(match action {
                PhyRxGainDcAction::ConfigureRegisters { enabled } => {
                    PhyRxGainDcCompletion::RegistersConfigured { enabled }
                }
                PhyRxGainDcAction::Rfpll(action) => PhyRxGainDcCompletion::Rfpll(match action {
                    R::StartChannelSwitch {
                        frequency_index,
                        crystal_selector,
                    } => RC::ChannelSwitchStarted {
                        frequency_index,
                        crystal_selector,
                    },
                    R::ClearChannelSwitch => RC::ChannelSwitchCleared,
                    R::ReadChannelReady { .. } => RC::ChannelReadyObserved { ready: true },
                    R::ConfigureNrx { frequency_mhz } => RC::NrxConfigured { frequency_mhz },
                    R::DelayMicros(micros) => RC::DelayElapsed(micros),
                    other => panic!("unexpected channel prefix {other:?}"),
                }),
                PhyRxGainDcAction::ConfigurePbusDebugMode => {
                    PhyRxGainDcCompletion::PbusDebugModeConfigured
                }
                PhyRxGainDcAction::ForcePbus { bank, transaction } => {
                    PhyRxGainDcCompletion::PbusCompleted { bank, transaction }
                }
                PhyRxGainDcAction::ConfigureClock { clock, enabled } => {
                    PhyRxGainDcCompletion::ClockConfigured { clock, enabled }
                }
                PhyRxGainDcAction::ReadPbus { selector, path } => PhyRxGainDcCompletion::PbusRead {
                    selector,
                    path,
                    value: 0,
                },
                PhyRxGainDcAction::I2c(action) => PhyRxGainDcCompletion::I2c(match action {
                    I::Read { address } => IC::Read { address, value: 0 },
                    I::Write { address, .. } => IC::Written { address },
                    other => panic!("unexpected I2C prefix {other:?}"),
                }),
                PhyRxGainDcAction::DelayMicros { phase, micros } => {
                    PhyRxGainDcCompletion::DelayElapsed { phase, micros }
                }
                PhyRxGainDcAction::Calibration(action) => {
                    use crate::rx::gain_calibration::{
                        PhyRxDcCalibrationAction as A, PhyRxDcCalibrationCompletion as C,
                    };
                    PhyRxGainDcCompletion::Calibration(match action {
                        A::PrepareControlRestore => C::ControlRestorePrepared,
                        A::ReadPbus { selector, path } => C::PbusRead {
                            selector,
                            path,
                            value: 0,
                        },
                        other => panic!("unexpected calibration prefix {other:?}"),
                    })
                }
                other => panic!("unexpected DC prefix {other:?}"),
            }),
            other => panic!("unexpected root prefix {other:?}"),
        };
        root.advance(completion).unwrap();
    }
    panic!("prefix did not reach minimum");
}

#[test]
fn short_rx_commands_share_a_poll_until_the_entry_budget_or_a_longer_atom() {
    for (command_micros, expected) in [(9, 3), (15, 2), (20, 1)] {
        let clock = Rc::new(VirtualClock::<1>::new());
        let mut io = Io {
            rx_calibration_clock: Some(clock.clone()),
            rx_calibration_micros: command_micros,
            ..Io::default()
        };
        let atoms = io.atoms.clone();
        let started = clock.now();
        let mut transition = root_at_command(true);
        let mut scratch = crate::executor::step::rx_gain::Scratch::default();
        let mut execution = crate::tracking::observation::RxGainExecution::default();
        let (waker, _) = context();
        let mut cx = Context::from_waker(&waker);
        {
            let mut job = Job::rx_gain(
                &mut transition,
                &mut scratch,
                &mut execution,
                clock.now(),
                true,
            )
            .expect("clock fits");
            let mut future = core::pin::pin!(drive_calibration::<TestDelay, _, _>(
                &mut job,
                &mut io,
                clock.as_ref(),
                None
            ));
            assert!(future.as_mut().poll(&mut cx).is_pending());
        }
        assert_eq!(atoms.get(), expected);
        assert_eq!(
            clock.now().saturating_duration_since(started).as_micros(),
            command_micros * expected as u64
        );
        assert!(
            transition.terminal().is_none(),
            "drop retains the live coefficient owner"
        );
    }
}

#[test]
fn root_control_preparation_yields_before_hardware_and_keeps_the_owner_uncommitted() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let atoms = io.atoms.clone();
    let mut transition = PhyRxGainInitTransition::new(dc_parameters());
    let before = transition;
    let mut scratch = crate::executor::step::rx_gain::Scratch::default();
    let mut execution = crate::tracking::observation::RxGainExecution::default();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut job = Job::rx_gain(
            &mut transition,
            &mut scratch,
            &mut execution,
            clock.now(),
            true,
        )
        .expect("clock fits");
        let mut future = core::pin::pin!(drive_calibration::<TestDelay, _, _>(
            &mut job, &mut io, &clock, None
        ));
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(atoms.get(), 0, "construction performs no hardware access");
    }
    assert_eq!(
        transition, before,
        "dropping a prepared command cannot commit the model"
    );
    assert!(scratch.binding.is_some());
    {
        let mut job = Job::rx_gain(
            &mut transition,
            &mut scratch,
            &mut execution,
            clock.now(),
            true,
        )
        .expect("clock fits");
        let mut future = core::pin::pin!(drive_calibration::<TestDelay, _, _>(
            &mut job, &mut io, &clock, None
        ));
        assert!(future.as_mut().poll(&mut cx).is_pending());
    }
    assert_eq!(atoms.get(), 1);
    assert_eq!(transition.phase(), Some(crate::rx::gain::Phase::Dc));
    assert!(transition.terminal().is_none());
    assert!(scratch.binding.is_none());
    assert!(scratch.completion.is_none());
}

#[test]
fn i2c_preparation_yields_before_the_whole_transaction_and_model_commit() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let atoms = io.atoms.clone();
    let mut transition = root_at_i2c();
    let before = transition;
    let mut scratch = crate::executor::step::rx_gain::Scratch::default();
    let mut execution = crate::tracking::observation::RxGainExecution::default();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut job = Job::rx_gain(
            &mut transition,
            &mut scratch,
            &mut execution,
            clock.now(),
            false,
        )
        .expect("clock fits");
        let mut future = core::pin::pin!(drive_calibration::<TestDelay, _, _>(
            &mut job, &mut io, &clock, None
        ));
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(atoms.get(), 0, "preparation performs no hardware access");
    }
    assert_eq!(
        transition, before,
        "drop before command leaves the model unchanged"
    );
    assert!(io.actions.is_empty());
    {
        let mut job = Job::rx_gain(
            &mut transition,
            &mut scratch,
            &mut execution,
            clock.now(),
            false,
        )
        .expect("clock fits");
        let mut future = core::pin::pin!(drive_calibration::<TestDelay, _, _>(
            &mut job, &mut io, &clock, None
        ));
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(
            atoms.get(),
            1,
            "one whole I2C command completes after resumption"
        );
    }
    assert_ne!(transition, before);
    assert_eq!(io.actions.len(), 2, "start and answer share the same poll");
    assert!(scratch.binding.is_none());
    assert!(scratch.completion.is_none());
}

#[test]
fn staged_i2c_failure_keeps_the_parent_uncommitted() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io {
        rx_i2c_failure: true,
        ..Io::default()
    };
    let mut transition = root_at_i2c();
    let before = transition;
    let mut scratch = crate::executor::step::rx_gain::Scratch::default();
    let mut execution = crate::tracking::observation::RxGainExecution::default();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut job = Job::rx_gain(
            &mut transition,
            &mut scratch,
            &mut execution,
            clock.now(),
            false,
        )
        .expect("clock fits");
        let mut future = core::pin::pin!(drive_calibration::<TestDelay, _, _>(
            &mut job, &mut io, &clock, None
        ));
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(
            future.as_mut().poll(&mut cx),
            Poll::Ready(Err(Failure::Binding))
        );
    }
    assert_eq!(transition, before);
    assert!(scratch.completion.is_none());
    assert!(io.actions.is_empty());
}

#[test]
fn first_and_repeated_i2c_polls_are_measured_separately_in_one_phase() {
    let clock = Rc::new(VirtualClock::<1>::new());
    let mut io = Io {
        rx_i2c_clock: Some(clock.clone()),
        ..Io::default()
    };
    let mut transition = root_at_i2c();
    let mut scratch = crate::executor::step::rx_gain::Scratch::default();
    let mut execution = crate::tracking::observation::RxGainExecution::default();
    let mut timing = EngineTiming {
        total_micros: 123,
        ..EngineTiming::default()
    };
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut job = Job::rx_gain(
            &mut transition,
            &mut scratch,
            &mut execution,
            clock.now(),
            true,
        )
        .expect("clock fits");
        let mut future = core::pin::pin!(drive_calibration::<TestDelay, _, _>(
            &mut job,
            &mut io,
            clock.as_ref(),
            Some(&mut timing)
        ));
        for _ in 0..4 {
            assert!(future.as_mut().poll(&mut cx).is_pending());
        }
    }
    assert_eq!(execution.i2c_samples, [1, 1]);
    assert_eq!(execution.i2c_poll_micros, [[0, 0], [10, 3]]);
    assert_eq!(
        timing.total_micros, 136,
        "prior phase totals are excluded from action samples"
    );
    assert!(!timing.invalid);
}
