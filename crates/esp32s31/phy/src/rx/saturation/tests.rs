use super::*;
use crate::analog::pbus::PhyPbusForceTest;

#[test]
fn transition_reproduces_all_pbus_commands_and_async_edges() {
    let expected = [
        PhyPbusForceTest::new(4, 1, 0),
        PhyPbusForceTest::new(4, 2, 1),
        PhyPbusForceTest::new(5, 1, 0),
        PhyPbusForceTest::new(0, 1, 0x40),
        PhyPbusForceTest::new(0, 2, 0xbf),
        PhyPbusForceTest::new(1, 1, 0x189),
        PhyPbusForceTest::new(1, 2, 0),
        PhyPbusForceTest::new(2, 1, 0x100),
        PhyPbusForceTest::new(3, 1, 0x100),
        PhyPbusForceTest::new(2, 2, 0x100),
        PhyPbusForceTest::new(3, 2, 0x100),
    ];
    let mut transition = PhyRxSaturationTransition::new(0xbf);
    assert_eq!(
        transition.action(),
        PhyRxSaturationAction::ConfigureDebugMode
    );
    transition
        .advance(PhyRxSaturationCompletion::DebugModeConfigured)
        .unwrap();
    for transaction in expected {
        assert_eq!(
            transition.action(),
            PhyRxSaturationAction::ForcePbus(transaction)
        );
        transition
            .advance(PhyRxSaturationCompletion::PbusCompleted(transaction))
            .unwrap();
    }
    assert_eq!(
        transition.action(),
        PhyRxSaturationAction::DelayMicros {
            micros: PHY_RX_SATURATION_DELAY_MICROS,
        }
    );
    transition
        .advance(PhyRxSaturationCompletion::DelayElapsed {
            micros: PHY_RX_SATURATION_DELAY_MICROS,
        })
        .unwrap();
    for sample_index in 0..PHY_RX_SATURATION_SAMPLE_COUNT {
        assert_eq!(
            transition.action(),
            PhyRxSaturationAction::SampleStatus {
                sample_index,
                samples: PHY_RX_SATURATION_SAMPLE_COUNT,
            }
        );
        transition
            .advance(PhyRxSaturationCompletion::StatusSampled {
                sample_index,
                active: sample_index < 7,
            })
            .unwrap();
    }
    assert_eq!(
        transition.action(),
        PhyRxSaturationAction::ConfigureWorkMode
    );
    transition
        .advance(PhyRxSaturationCompletion::WorkModeConfigured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRxSaturationAction::Complete(PhyRxSaturationOutcome::Measured {
            saturated_samples: 7,
            samples: 100,
        })
    );
}

#[test]
fn sample_rejects_stale_index() {
    let mut transition = PhyRxSaturationTransition::new(0);
    transition
        .advance(PhyRxSaturationCompletion::DebugModeConfigured)
        .unwrap();
    for transaction in [
        PhyPbusForceTest::new(4, 1, 0),
        PhyPbusForceTest::new(4, 2, 1),
        PhyPbusForceTest::new(5, 1, 0),
        PhyPbusForceTest::new(0, 1, 0x40),
        PhyPbusForceTest::new(0, 2, 0),
        PhyPbusForceTest::new(1, 1, 0x189),
        PhyPbusForceTest::new(1, 2, 0),
        PhyPbusForceTest::new(2, 1, 0x100),
        PhyPbusForceTest::new(3, 1, 0x100),
        PhyPbusForceTest::new(2, 2, 0x100),
        PhyPbusForceTest::new(3, 2, 0x100),
    ] {
        transition
            .advance(PhyRxSaturationCompletion::PbusCompleted(transaction))
            .unwrap();
    }
    transition
        .advance(PhyRxSaturationCompletion::DelayElapsed { micros: 5 })
        .unwrap();
    assert_eq!(
        transition.advance(PhyRxSaturationCompletion::StatusSampled {
            sample_index: 1,
            active: true,
        }),
        Err(PhyRxSaturationTransitionError::InvalidCapture)
    );
}

#[test]
fn sample_binding_accepts_only_exact_one_shot_poll_action() {
    let action = PhyRxSaturationAction::SampleStatus {
        sample_index: 42,
        samples: PHY_RX_SATURATION_SAMPLE_COUNT,
    };
    assert!(PhyRxSaturationSampleBinding::new(action).is_ok());
    assert_eq!(
        PhyRxSaturationSampleBinding::new(PhyRxSaturationAction::ConfigureDebugMode),
        Err(PhyRxSaturationSampleBindingError::NotStatusSample)
    );
}

#[test]
fn timeout_still_restores_work_mode_before_terminal_state() {
    let transaction = PhyPbusForceTest::new(4, 1, 0);
    let mut transition = PhyRxSaturationTransition::new(0);
    transition
        .advance(PhyRxSaturationCompletion::DebugModeConfigured)
        .unwrap();
    transition
        .advance(PhyRxSaturationCompletion::PbusTimedOut(transaction))
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRxSaturationAction::ConfigureWorkMode
    );
    transition
        .advance(PhyRxSaturationCompletion::WorkModeConfigured)
        .unwrap();
    assert_eq!(
        transition.action(),
        PhyRxSaturationAction::Complete(PhyRxSaturationOutcome::PbusTimedOut(transaction))
    );
}

#[test]
fn external_lowering_covers_each_saturation_operation_class() {
    assert!(matches!(
        PhyRxSaturationExternalBinding::lower(PhyRxSaturationAction::ConfigureDebugMode),
        Ok(PhyRxSaturationExternalBinding::Mmio(_))
    ));
    assert!(matches!(
        PhyRxSaturationExternalBinding::lower(PhyRxSaturationAction::ForcePbus(
            PhyPbusForceTest::new(4, 1, 0)
        )),
        Ok(PhyRxSaturationExternalBinding::Pbus(_))
    ));
    assert!(matches!(
        PhyRxSaturationExternalBinding::lower(PhyRxSaturationAction::DelayMicros {
            micros: PHY_RX_SATURATION_DELAY_MICROS,
        }),
        Ok(PhyRxSaturationExternalBinding::Timer(_))
    ));
    assert!(matches!(
        PhyRxSaturationExternalBinding::lower(PhyRxSaturationAction::SampleStatus {
            sample_index: 0,
            samples: PHY_RX_SATURATION_SAMPLE_COUNT,
        }),
        Ok(PhyRxSaturationExternalBinding::Sample(_))
    ));
    assert!(matches!(
        PhyRxSaturationExternalBinding::lower(PhyRxSaturationAction::Complete(
            PhyRxSaturationOutcome::CaptureTimedOut
        )),
        Err(PhyRxSaturationBindingError::UnsupportedAction)
    ));
}
