use super::*;
use crate::{
    analog::pbus::PhyForceTxRxAction as Force,
    state::{PhyConfig, PhyState},
    tracking::{
        parameters::{
            PhyCalibrationTrackClass as Class, PhyParamTrackingCompletion as Done,
            PhyParamTrackingPolicy, PhyParamTrackingTransition, PhyParamTrackingTxPowerTransition,
        },
        power::{PhyTxPowerTrackingAction as Action, PhyTxPowerTrackingCompletion as Completion},
    },
};

impl Io {
    pub(in crate::executor::step) fn power_gain_base(
        &mut self,
        action: Action,
    ) -> Result<u8, Failure> {
        self.atom();
        if self.power_failure == Some(self.power_actions.len()) {
            return Err(Failure::Binding);
        }
        self.power_actions.push(action);
        Ok(0xf3)
    }
    pub(in crate::executor::step) fn power_gain_entry(
        &mut self,
        entry: oer_esp32s31_hal::types::PhyGainMemoryEntry,
    ) -> Result<(), Failure> {
        self.atom();
        if self.power_gain_failure == Some(self.power_gain_entries.len()) {
            return Err(Failure::Binding);
        }
        self.power_gain_entries.push(entry);
        Ok(())
    }
    pub(in crate::executor::step) fn power(
        &mut self,
        binding: crate::tracking::power::ControlBinding,
    ) -> Result<Completion, Failure> {
        let action = binding.action();
        if !matches!(action, Action::ForceTxRx(Force::DelayMicros { .. })) {
            self.atom();
        }
        if self.power_failure == Some(self.power_actions.len()) {
            return Err(Failure::Binding);
        }
        self.power_actions.push(action);
        Ok(match action {
            Action::SetBbpllCalibration { enabled } => Completion::BbpllCalibrationSet { enabled },
            Action::ForceTxRx(action) => Completion::ForceTxRx(match action {
                Force::Configure { enabled, phase } => {
                    crate::analog::pbus::PhyForceTxRxCompletion::Configured { enabled, phase }
                }
                Force::DelayMicros {
                    enabled,
                    completed_phase,
                    micros,
                } => crate::analog::pbus::PhyForceTxRxCompletion::DelayElapsed {
                    enabled,
                    completed_phase,
                    micros,
                },
                Force::Complete { .. } => panic!("terminal force cannot reach hardware"),
            }),
            Action::RegenerateWifiGain { channel, gain_base } => {
                Completion::WifiGainRegenerated { channel, gain_base }
            }
            Action::RegenerateBluetoothIeee802154Gain { gain_base } => {
                Completion::BluetoothIeee802154GainRegenerated { gain_base }
            }
            Action::Complete(_) => panic!("terminal power cannot reach hardware"),
        })
    }
}

fn child(
    state: &mut PhyState,
    class: Class,
    enabled: bool,
) -> PhyParamTrackingTxPowerTransition<'_> {
    let mut policy = PhyParamTrackingPolicy::for_registered_state(state);
    policy.rfpll_cap_tracking_enabled = false;
    policy.bluetooth_ieee802154_power_tracking_enabled = enabled;
    let mut parent = PhyParamTrackingTransition::new(class.clients(), policy);
    parent.advance(Done::EnteredCritical).expect("enter owner");
    if class == Class::Wifi {
        parent
            .advance(Done::WifiI2cTracked)
            .expect("complete preceding T3");
    }
    parent
        .begin_tx_power_tracking(state)
        .expect("power selected")
}

fn state(temperature: i16) -> PhyState {
    let mut state = PhyState::new(PhyConfig::production());
    let dco = crate::tx::dc_offset::PhyTxDcOutcome {
        dco: [
            [1, 2, 3, 4],
            [5, 6, 7, 8],
            [9, 10, 11, 12],
            [13, 14, 15, 16],
            [17, 18, 19, 20],
        ],
    };
    state.apply_tx_dc_outcome(dco);
    state.apply_bluetooth_tx_dc_outcome(dco);
    state.apply_temperature_outcome(PhyTemperatureOutcome {
        temperature,
        sensor_index: 3,
        next_dac: 4,
    });
    state
}

#[test]
fn both_classes_yield_between_atoms_keep_force_settles_and_commit_after_bbpll_off() {
    for class in [Class::Wifi, Class::BluetoothIeee802154] {
        let clock = VirtualClock::<1>::new();
        let mut io = Io::default();
        let atoms = io.atoms.clone();
        let mut state = state(60);
        let before = state.tx_power_tracking_parameters(false);
        let gain_base = child(&mut state, class, true)
            .parts()
            .0
            .decision()
            .gain_base;
        let expected = expected_records(&state, class, gain_base);
        let (waker, wakes) = context();
        let mut cx = Context::from_waker(&waker);
        let (result, timing, _gain) = {
            let mut future =
                core::pin::pin!(super::super::power::run_param_tx_power::<TestDelay, _, _>(
                    &clock,
                    child(&mut state, class, true),
                    &mut io,
                    true
                ));
            let mut polls = 0;
            loop {
                polls += 1;
                assert!(polls <= if class == Class::Wifi { 40 } else { 24 });
                let before = atoms.get();
                let result = future.as_mut().poll(&mut cx);
                assert!(
                    atoms.get() - before <= 1,
                    "one complete atom per caller poll"
                );
                if let Poll::Ready(result) = result {
                    break result;
                }
                assert!(wakes.0.load(std::sync::atomic::Ordering::Relaxed) >= polls);
                clock
                    .advance(Duration::from_micros(1))
                    .expect("virtual time fits");
            }
        };
        assert!(matches!(
            result,
            Ok(Done::WifiTxPowerTracked {
                gain_updated: true,
                ..
            } | Done::BluetoothIeee802154TxPowerTracked {
                gain_updated: true,
                ..
            })
        ));
        assert_eq!(
            (
                timing.polls,
                timing.pending,
                timing.resumes,
                timing.ready_edges
            ),
            if class == Class::Wifi {
                (40, 39, 0, 33)
            } else {
                (24, 23, 0, 17)
            }
        );
        assert_eq!(clock.next_deadline(), None, "Runnable uses the waker only");
        assert_ne!(state.tx_power_tracking_parameters(false), before);
        assert_eq!(
            io.power_gain_entries, expected,
            "every complete encoded record in order, including index wrapping"
        );
        assert_eq!(
            io.power_actions.first(),
            Some(&Action::SetBbpllCalibration { enabled: true })
        );
        assert_eq!(
            io.power_actions.last(),
            Some(&Action::SetBbpllCalibration { enabled: false })
        );
        let force: Vec<_> = io
            .power_actions
            .iter()
            .filter_map(|action| match action {
                Action::ForceTxRx(force) => Some(*force),
                _ => None,
            })
            .collect();
        let expected: Vec<_> = [true, false]
            .into_iter()
            .flat_map(|enabled| {
                (0..2).flat_map(move |phase| {
                    [
                        Force::Configure { enabled, phase },
                        Force::DelayMicros {
                            enabled,
                            completed_phase: phase,
                            micros: 1,
                        },
                    ]
                })
            })
            .collect();
        assert_eq!(force, expected);
        assert!(matches!(
            io.power_actions.get(5),
            Some(
                Action::RegenerateWifiGain { .. }
                    | Action::RegenerateBluetoothIeee802154Gain { .. }
            )
        ));
    }
}

#[test]
fn failure_or_drop_after_publication_never_commits_the_power_reference() {
    for class in [Class::Wifi, Class::BluetoothIeee802154] {
        for failure in [Some(0), Some(5), Some(10), None] {
            let clock = VirtualClock::<1>::new();
            let mut io = Io {
                power_failure: failure,
                ..Io::default()
            };
            let mut state = state(60);
            let before = state.tx_power_tracking_parameters(false);
            let (waker, _) = context();
            let mut cx = Context::from_waker(&waker);
            {
                let mut future =
                    core::pin::pin!(super::super::power::run_param_tx_power::<TestDelay, _, _>(
                        &clock,
                        child(&mut state, class, true),
                        &mut io,
                        false
                    ));
                // Even after every gain record and BBPLL-off, only the terminal
                // poll may mint the identity-bound parent acknowledgement.
                for _ in 0..if class == Class::Wifi { 39 } else { 23 } {
                    if let Poll::Ready((result, _, _)) = future.as_mut().poll(&mut cx) {
                        assert_eq!(result, Err(Failure::Binding));
                        assert!(failure.is_some());
                        break;
                    }
                }
            }
            assert_eq!(state.tx_power_tracking_parameters(false), before);
        }
    }
}

#[test]
fn disabled_or_unchanged_power_finishes_without_hardware_or_pending() {
    for (enabled, temperature) in [(false, 60), (true, 0)] {
        let clock = VirtualClock::<1>::new();
        let mut io = Io::default();
        let mut state = state(temperature);
        let (waker, _) = context();
        let mut cx = Context::from_waker(&waker);
        {
            let mut future =
                core::pin::pin!(super::super::power::run_param_tx_power::<TestDelay, _, _>(
                    &clock,
                    child(&mut state, Class::BluetoothIeee802154, enabled),
                    &mut io,
                    true
                ));
            let Poll::Ready((
                Ok(Done::BluetoothIeee802154TxPowerTracked {
                    gain_updated: false,
                    ..
                }),
                timing,
                _,
            )) = future.as_mut().poll(&mut cx)
            else {
                panic!("no-op is ready");
            };
            assert_eq!(
                (timing.polls, timing.pending, timing.ready_edges),
                (1, 0, 0)
            );
        }
        assert!(io.power_actions.is_empty());
    }
}

fn expected_records(
    state: &PhyState,
    class: Class,
    gain_base: i8,
) -> Vec<oer_esp32s31_hal::types::PhyGainMemoryEntry> {
    use crate::hardware::{packed_halfword, tx_baseband_gain_index};
    match class {
        Class::Wifi => {
            let image = state
                .wifi_tracking_gain_image(state.current_wifi_channel(), gain_base)
                .unwrap();
            (0..32)
                .map(|i| {
                    let gain_64 = packed_halfword(&image.output_64, i);
                    let seed_index = tx_baseband_gain_index(gain_64) * 4;
                    let seed = |index| {
                        if index < 12 {
                            packed_halfword(&image.seed, index)
                        } else {
                            packed_halfword(&image.output_32, index - 12)
                        }
                    };
                    oer_esp32s31_hal::types::PhyGainMemoryEntry::transmit(
                        0xf3_u8.wrapping_add(i as u8),
                        packed_halfword(&image.output_72, i),
                        gain_64,
                        (image.output_32[i / 4] >> ((i % 4) * 8)) as u8,
                        [
                            seed(seed_index),
                            seed(seed_index + 1),
                            seed(seed_index + 2),
                            seed(seed_index + 3),
                        ],
                        image.config,
                    )
                })
                .collect()
        }
        Class::BluetoothIeee802154 => {
            let image = state.bluetooth_ieee802154_tracking_gain_image(gain_base);
            (0..16)
                .map(|i| {
                    let gain_64 = image.output_64[i];
                    let index = tx_baseband_gain_index(gain_64) * 4;
                    oer_esp32s31_hal::types::PhyGainMemoryEntry::transmit(
                        0xf3_u8.wrapping_add(32).wrapping_add(i as u8),
                        image.output_72[i],
                        gain_64,
                        image.output_32[i],
                        [
                            packed_halfword(&image.seed, index),
                            packed_halfword(&image.seed, index + 1),
                            packed_halfword(&image.seed, index + 2),
                            packed_halfword(&image.seed, index + 3),
                        ],
                        image.config,
                    )
                })
                .collect()
        }
    }
}

#[test]
fn failure_or_drop_between_gain_records_never_commits() {
    for class in [Class::Wifi, Class::BluetoothIeee802154] {
        let len = if class == Class::Wifi { 32 } else { 16 };
        for position in [0, len / 2, len - 1] {
            for fail in [true, false] {
                let clock = VirtualClock::<1>::new();
                let mut io = Io {
                    power_gain_failure: fail.then_some(position),
                    ..Default::default()
                };
                let mut state = state(60);
                let before = state.tx_power_tracking_parameters(false);
                let (waker, _) = context();
                let mut cx = Context::from_waker(&waker);
                {
                    let mut future =
                        core::pin::pin!(
                            super::super::power::run_param_tx_power::<TestDelay, _, _>(
                                &clock,
                                child(&mut state, class, true),
                                &mut io,
                                false
                            )
                        );
                    // Three setup atoms, one lower, then precisely position records.
                    for _ in 0..4 + position {
                        assert!(future.as_mut().poll(&mut cx).is_pending());
                    }
                    if fail {
                        assert!(matches!(
                            future.as_mut().poll(&mut cx),
                            Poll::Ready((Err(Failure::Binding), _, _))
                        ));
                    }
                }
                assert_eq!(io.power_gain_entries.len(), position);
                assert_eq!(state.tx_power_tracking_parameters(false), before);
            }
        }
    }
}
